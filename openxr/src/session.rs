use std::mem::MaybeUninit;
use std::{marker::PhantomData, ptr, sync::Arc};

use crate::*;

/// A rendering session using a particular graphics API `G`
pub struct Session<G: Graphics> {
    pub(crate) inner: Arc<SessionInner>,
    _marker: PhantomData<G>,
}

impl<G: Graphics> Session<G> {
    /// Take ownership of an existing session handle
    ///
    /// # Safety
    ///
    /// `handle` must be a valid session handle associated with `instance` which is not currently
    /// inside a frame and was created for graphics API `G`.
    #[inline]
    pub unsafe fn from_raw(
        instance: Instance,
        handle: sys::Session,
    ) -> (Self, FrameWaiter, FrameStream<G>) {
        let session = Self {
            inner: Arc::new(SessionInner {
                instance: instance.clone(),
                handle,
            }),
            _marker: PhantomData,
        };
        (
            session.clone(),
            FrameWaiter::new(session.clone()),
            FrameStream::new(session),
        )
    }

    /// Access the raw session handle
    #[inline]
    pub fn as_raw(&self) -> sys::Session {
        self.inner.handle
    }

    /// Access the `Instance` self is descended from
    #[inline]
    pub fn instance(&self) -> &Instance {
        &self.inner.instance
    }

    /// Set the debug name of this `Session`, if `XR_EXT_debug_utils` is loaded
    #[inline]
    pub fn set_name(&mut self, name: &str) -> Result<()> {
        self.instance().set_name_raw(self.as_raw().into_raw(), name)
    }

    /// Request that the runtime show the application's rendered output to the user
    #[inline]
    pub fn begin(&self, ty: ViewConfigurationType) -> Result<sys::Result> {
        let info = sys::SessionBeginInfo {
            ty: sys::SessionBeginInfo::TYPE,
            next: ptr::null(),
            primary_view_configuration_type: ty,
        };
        unsafe { cvt((self.fp().begin_session)(self.as_raw(), &info)) }
    }

    /// Request a transition to `SessionState::STOPPING` so that `end` may be called.
    #[inline]
    pub fn request_exit(&self) -> Result<()> {
        unsafe {
            cvt((self.fp().request_exit_session)(self.as_raw()))?;
        }
        Ok(())
    }

    /// Terminate a session in the `SessionState::STOPPING` state
    ///
    /// See `request_exit` for active sessions.
    #[inline]
    pub fn end(&self) -> Result<sys::Result> {
        unsafe { cvt((self.fp().end_session)(self.as_raw())) }
    }

    #[inline]
    pub fn reference_space_bounds_rect(&self, ty: ReferenceSpaceType) -> Result<Option<Extent2Df>> {
        unsafe {
            let mut out = MaybeUninit::uninit();
            let status = cvt((self.fp().get_reference_space_bounds_rect)(
                self.as_raw(),
                ty,
                out.as_mut_ptr(),
            ))?;
            Ok(if status == sys::Result::SPACE_BOUNDS_UNAVAILABLE {
                None
            } else {
                Some(out.assume_init())
            })
        }
    }

    /// Enumerate the set of reference space types supported for this session
    ///
    /// Constant for the lifetime of the session.
    #[inline]
    pub fn enumerate_reference_spaces(&self) -> Result<Vec<ReferenceSpaceType>> {
        get_arr(|cap, count, buf| unsafe {
            (self.fp().enumerate_reference_spaces)(self.as_raw(), cap, count, buf)
        })
    }

    /// Creates a `Space` based on a chosen reference space
    pub fn create_reference_space(
        &self,
        reference_space_type: ReferenceSpaceType,
        pose_in_reference_space: Posef,
    ) -> Result<Space> {
        let info = sys::ReferenceSpaceCreateInfo {
            ty: sys::ReferenceSpaceCreateInfo::TYPE,
            next: ptr::null(),
            reference_space_type,
            pose_in_reference_space,
        };
        let mut out = sys::Space::NULL;
        unsafe {
            cvt((self.fp().create_reference_space)(
                self.as_raw(),
                &info,
                &mut out,
            ))?;
            Ok(Space::reference_from_raw(self.clone(), out))
        }
    }

    /// Enumerate texture formats supported by the current session
    ///
    /// The type of formats returned is dependent on the graphics API for which the session was
    /// created.
    #[inline]
    pub fn enumerate_swapchain_formats(&self) -> Result<Vec<G::Format>> {
        let raw = get_arr(|capacity, count, buf| unsafe {
            (self.fp().enumerate_swapchain_formats)(self.as_raw(), capacity, count, buf)
        })?;
        Ok(raw.into_iter().map(G::raise_format).collect())
    }

    #[inline]
    pub fn create_swapchain(&self, info: &SwapchainCreateInfo<G>) -> Result<Swapchain<G>> {
        let mut out = sys::Swapchain::NULL;
        let info = sys::SwapchainCreateInfo {
            ty: sys::SwapchainCreateInfo::TYPE,
            next: ptr::null(),
            create_flags: info.create_flags,
            usage_flags: info.usage_flags,
            format: G::lower_format(info.format),
            sample_count: info.sample_count,
            width: info.width,
            height: info.height,
            face_count: info.face_count,
            array_size: info.array_size,
            mip_count: info.mip_count,
        };
        unsafe {
            cvt((self.fp().create_swapchain)(self.as_raw(), &info, &mut out))?;
            Ok(Swapchain::from_raw(self.clone(), out))
        }
    }

    /// Get the view and projection info for a particular display time
    ///
    /// When rendering, this should be called as late as possible before the GPU accesses it to
    /// provide the most accurate possible poses.
    #[inline]
    pub fn locate_views(
        &self,
        view_configuration_type: ViewConfigurationType,
        display_time: Time,
        space: &Space,
    ) -> Result<(ViewStateFlags, Vec<View>)> {
        let info = sys::ViewLocateInfo {
            ty: sys::ViewLocateInfo::TYPE,
            next: ptr::null(),
            view_configuration_type,
            display_time,
            space: space.as_raw(),
        };
        let (flags, raw) = unsafe {
            let mut out = sys::ViewState::out(ptr::null_mut());
            let raw = get_arr_init(sys::View::out(ptr::null_mut()), |cap, count, buf| {
                (self.fp().locate_views)(
                    self.as_raw(),
                    &info,
                    out.as_mut_ptr(),
                    cap,
                    count,
                    buf as _,
                )
            })?;
            (out.assume_init().view_state_flags, raw)
        };
        Ok((
            flags,
            raw.into_iter()
                .map(|x| {
                    let x = unsafe { x.assume_init() };
                    View {
                        pose: x.pose,
                        fov: x.fov,
                    }
                })
                .collect(),
        ))
    }

    /// Get the suggested interaction profile in use for a top level user path
    ///
    /// May be NULL.
    #[inline]
    pub fn current_interaction_profile(&self, top_level_user_path: Path) -> Result<Path> {
        unsafe {
            let mut out = sys::InteractionProfileState::out(ptr::null_mut());
            cvt((self.fp().get_current_interaction_profile)(
                self.as_raw(),
                top_level_user_path,
                out.as_mut_ptr(),
            ))?;
            Ok(out.assume_init().interaction_profile)
        }
    }

    /// Enable use of action sets with a session
    ///
    /// Once attached, action sets become immutable.
    #[inline]
    pub fn attach_action_sets(&self, sets: &[&ActionSet]) -> Result<()> {
        let sets = sets.iter().map(|x| x.as_raw()).collect::<Vec<_>>();
        let info = sys::SessionActionSetsAttachInfo {
            ty: sys::SessionActionSetsAttachInfo::TYPE,
            next: ptr::null(),
            count_action_sets: sets.len() as u32,
            action_sets: sets.as_ptr(),
        };
        unsafe {
            cvt((self.fp().attach_session_action_sets)(self.as_raw(), &info))?;
        }
        Ok(())
    }

    /// Designate active input actions and update their states
    #[inline]
    pub fn sync_actions(&self, action_sets: &[ActiveActionSet<'_>]) -> Result<()> {
        let info = sys::ActionsSyncInfo {
            ty: sys::ActionsSyncInfo::TYPE,
            next: ptr::null(),
            count_active_action_sets: action_sets.len() as u32,
            active_action_sets: action_sets.as_ptr() as _,
        };
        unsafe {
            cvt((self.fp().sync_actions)(self.as_raw(), &info))?;
        }
        Ok(())
    }

    /// Get a name for the input source in the current system locale
    #[inline]
    pub fn input_source_localized_name(
        &self,
        source: Path,
        which_components: InputSourceLocalizedNameFlags,
    ) -> Result<String> {
        let info = sys::InputSourceLocalizedNameGetInfo {
            ty: sys::InputSourceLocalizedNameGetInfo::TYPE,
            next: ptr::null(),
            source_path: source,
            which_components,
        };
        get_str(|cap, count, buf| unsafe {
            (self.fp().get_input_source_localized_name)(self.as_raw(), &info, cap, count, buf)
        })
    }

    /// Get a mesh describing the visible area of a view
    ///
    /// Requires KHR_visibility_mask. Useful to skip shading fragments the user can't see.
    ///
    /// See also the `VisibilityMaskChangedKHR` event.
    #[inline]
    pub fn get_visibility_mask_khr(
        &self,
        view_configuration_type: ViewConfigurationType,
        view_index: u32,
        visibility_mask_type: VisibilityMaskTypeKHR,
    ) -> Result<VisibilityMask> {
        let mut info = sys::VisibilityMaskKHR {
            ty: sys::VisibilityMaskKHR::TYPE,
            next: ptr::null_mut(),
            vertex_capacity_input: 0,
            vertex_count_output: 0,
            vertices: ptr::null_mut(),
            index_capacity_input: 0,
            index_count_output: 0,
            indices: ptr::null_mut(),
        };
        unsafe {
            cvt((self.instance().visibility_mask().get_visibility_mask)(
                self.as_raw(),
                view_configuration_type,
                view_index,
                visibility_mask_type,
                &mut info,
            ))?;
            let mut out = VisibilityMask {
                vertices: Vec::with_capacity(info.vertex_count_output as usize),
                indices: Vec::with_capacity(info.index_count_output as usize),
            };
            loop {
                info.vertex_capacity_input = out.vertices.capacity() as u32;
                info.index_capacity_input = out.indices.capacity() as u32;
                match cvt((self.instance().visibility_mask().get_visibility_mask)(
                    self.as_raw(),
                    view_configuration_type,
                    view_index,
                    visibility_mask_type,
                    &mut info,
                )) {
                    Ok(_) => {
                        out.vertices.set_len(info.vertex_count_output as usize);
                        out.indices.set_len(info.index_count_output as usize);
                        return Ok(out);
                    }
                    Err(sys::Result::ERROR_SIZE_INSUFFICIENT) => {
                        out.vertices.reserve(
                            (info.vertex_count_output as usize)
                                .saturating_sub(out.vertices.capacity()),
                        );
                        out.indices.reserve(
                            (info.index_count_output as usize)
                                .saturating_sub(out.indices.capacity()),
                        );
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
        }
    }

    // Private helper
    #[inline]
    fn fp(&self) -> &raw::Instance {
        self.inner.instance.fp()
    }

    pub(crate) fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

/// Mesh obtained from `Session::get_visibility_mask`
#[derive(Clone)]
pub struct VisibilityMask {
    pub vertices: Vec<Vector2f>,
    pub indices: Vec<u32>,
}

impl<G: Graphics> Clone for Session<G> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

pub(crate) struct SessionInner {
    pub(crate) instance: Instance,
    pub(crate) handle: sys::Session,
}

impl Drop for SessionInner {
    fn drop(&mut self) {
        unsafe {
            (self.instance.fp().destroy_session)(self.handle);
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct SwapchainCreateInfo<G: Graphics> {
    pub create_flags: SwapchainCreateFlags,
    pub usage_flags: SwapchainUsageFlags,
    pub format: G::Format,
    pub sample_count: u32,
    pub width: u32,
    pub height: u32,
    pub face_count: u32,
    pub array_size: u32,
    pub mip_count: u32,
}

#[derive(Copy, Clone)]
pub struct View {
    pub pose: Posef,
    pub fov: Fovf,
}

#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct ActiveActionSet<'a> {
    _inner: sys::ActiveActionSet,
    _marker: PhantomData<&'a ActionSet>,
}

impl<'a> ActiveActionSet<'a> {
    #[inline]
    pub fn new(action_set: &'a ActionSet) -> Self {
        Self::with_subaction(action_set, Path::NULL)
    }

    #[inline]
    pub fn with_subaction(action_set: &'a ActionSet, subaction_path: Path) -> Self {
        Self {
            _inner: sys::ActiveActionSet {
                action_set: action_set.as_raw(),
                subaction_path,
            },
            _marker: PhantomData,
        }
    }
}

impl<'a> From<&'a ActionSet> for ActiveActionSet<'a> {
    fn from(x: &'a ActionSet) -> Self {
        Self::new(x)
    }
}

/// Handle for waiting to render a frame
pub struct FrameWaiter {
    session: Arc<SessionInner>,
}

impl FrameWaiter {
    fn new<G: Graphics>(session: Session<G>) -> Self {
        Self {
            session: session.inner,
        }
    }

    /// Block until rendering should begin, and return details to guide rendering
    #[inline]
    pub fn wait(&mut self) -> Result<FrameState> {
        let out = unsafe {
            let mut x = sys::FrameState::out(ptr::null_mut());
            cvt((self.session.instance.fp().wait_frame)(
                self.session.handle,
                ptr::null(),
                x.as_mut_ptr(),
            ))?;
            x.assume_init()
        };
        Ok(FrameState {
            predicted_display_time: out.predicted_display_time,
            predicted_display_period: out.predicted_display_period,
            should_render: out.should_render.into(),
        })
    }
}
