#[derive(Clone, Debug, PartialEq)]
pub struct PsmoveApiObservation {
    pub move_id: String,
    pub center_x_px: f32,
    pub center_y_px: f32,
    pub radius_px: f32,
    pub age_ms: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PsmoveApiCameraInfo {
    pub name: String,
    pub api: String,
    pub width: u32,
    pub height: u32,
    pub exposure: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsmoveApiTrackerError(pub String);

impl core::fmt::Display for PsmoveApiTrackerError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PsmoveApiTrackerError {}

#[cfg(target_os = "linux")]
mod linux {
    use super::{PsmoveApiCameraInfo, PsmoveApiObservation, PsmoveApiTrackerError};
    use core::ffi::{c_char, c_float, c_int, c_uchar, c_void};
    use std::ffi::CStr;

    const TRACKER_CALIBRATED: c_int = 2;

    #[repr(C)]
    struct CameraInfo {
        camera_name: *const c_char,
        camera_api: *const c_char,
        width: c_int,
        height: c_int,
    }

    #[link(name = "psmoveapi")]
    #[link(name = "psmoveapi_tracker")]
    unsafe extern "C" {
        fn psmove_count_connected() -> c_int;
        fn psmove_connect_by_id(id: c_int) -> *mut c_void;
        fn psmove_get_serial(controller: *mut c_void) -> *const c_char;
        fn psmove_disconnect(controller: *mut c_void);
        fn psmove_tracker_new_with_camera(camera: c_int) -> *mut c_void;
        fn psmove_tracker_free(tracker: *mut c_void);
        fn psmove_tracker_enable_color_observer(
            tracker: *mut c_void,
            controller: *mut c_void,
            red: c_uchar,
            green: c_uchar,
            blue: c_uchar,
        ) -> c_int;
        fn psmove_tracker_set_expected_color(
            tracker: *mut c_void,
            controller: *mut c_void,
            red: c_uchar,
            green: c_uchar,
            blue: c_uchar,
        ) -> bool;
        fn psmove_tracker_disable(tracker: *mut c_void, controller: *mut c_void);
        fn psmove_tracker_set_exposure(tracker: *mut c_void, exposure: c_float);
        fn psmove_tracker_get_exposure(tracker: *mut c_void) -> c_float;
        fn psmove_tracker_update_image(tracker: *mut c_void);
        fn psmove_tracker_update(tracker: *mut c_void, controller: *mut c_void) -> c_int;
        fn psmove_tracker_get_position(
            tracker: *mut c_void,
            controller: *mut c_void,
            x: *mut c_float,
            y: *mut c_float,
            radius: *mut c_float,
        ) -> c_int;
        fn psmove_tracker_get_camera_info(tracker: *mut c_void) -> *const CameraInfo;
    }

    struct Controller {
        handle: *mut c_void,
        move_id: String,
    }

    pub struct PsmoveApiTracker {
        tracker: *mut c_void,
        controllers: Vec<Controller>,
        colors: Vec<(String, [u8; 3])>,
        camera_info: PsmoveApiCameraInfo,
    }

    impl PsmoveApiTracker {
        pub fn open(
            camera_index: i32,
            exposure: f32,
            colors: &[(String, [u8; 3])],
        ) -> Result<Self, PsmoveApiTrackerError> {
            if !(0.0..=1.0).contains(&exposure) {
                return Err(PsmoveApiTrackerError(format!(
                    "PSMoveAPI exposure must be within 0..=1, got {exposure}"
                )));
            }
            let tracker = unsafe { psmove_tracker_new_with_camera(camera_index) };
            if tracker.is_null() {
                return Err(PsmoveApiTrackerError(format!(
                    "PSMoveAPI could not open camera index {camera_index}"
                )));
            }
            unsafe { psmove_tracker_set_exposure(tracker, exposure) };

            let mut result = Self {
                tracker,
                controllers: Vec::new(),
                colors: colors.to_vec(),
                camera_info: PsmoveApiCameraInfo {
                    name: "unknown".to_string(),
                    api: "unknown".to_string(),
                    width: 0,
                    height: 0,
                    exposure,
                },
            };
            result.observe_connected();
            result.camera_info = unsafe {
                let info = psmove_tracker_get_camera_info(tracker);
                if info.is_null() {
                    PsmoveApiCameraInfo {
                        exposure: psmove_tracker_get_exposure(tracker),
                        ..result.camera_info.clone()
                    }
                } else {
                    PsmoveApiCameraInfo {
                        name: text((*info).camera_name),
                        api: text((*info).camera_api),
                        width: (*info).width.max(0) as u32,
                        height: (*info).height.max(0) as u32,
                        exposure: psmove_tracker_get_exposure(tracker),
                    }
                }
            };
            Ok(result)
        }

        pub fn calibrate_connected(&mut self) -> usize {
            self.observe_connected()
        }

        pub fn observe_connected(&mut self) -> usize {
            let count = unsafe { psmove_count_connected() }.max(0);
            for index in 0..count {
                let handle = unsafe { psmove_connect_by_id(index) };
                if handle.is_null() {
                    continue;
                }
                let serial = unsafe { text(psmove_get_serial(handle)) };
                let move_id = format!("move-{}", serial.replace(':', "").to_ascii_lowercase());
                if self.controllers.iter().any(|controller| controller.move_id == move_id) {
                    unsafe { psmove_disconnect(handle) };
                    continue;
                }
                let Some((_, color)) = self.colors.iter().find(|(identity, _)| {
                    identity.eq_ignore_ascii_case(&move_id)
                }) else {
                    unsafe { psmove_disconnect(handle) };
                    continue;
                };
                let status = unsafe {
                    psmove_tracker_enable_color_observer(
                        self.tracker, handle, color[0], color[1], color[2]
                    )
                };
                if status == TRACKER_CALIBRATED {
                    self.controllers.push(Controller { handle, move_id });
                } else {
                    unsafe { psmove_disconnect(handle) };
                }
            }

            self.controllers.len()
        }

        pub fn set_expected_color(&mut self, move_id: &str, color: [u8; 3]) -> bool {
            let Some(controller) = self.controllers.iter().find(|controller| {
                controller.move_id.eq_ignore_ascii_case(move_id)
            }) else {
                return false;
            };
            unsafe {
                psmove_tracker_set_expected_color(
                    self.tracker,
                    controller.handle,
                    color[0],
                    color[1],
                    color[2],
                )
            }
        }

        pub fn camera_info(&self) -> &PsmoveApiCameraInfo {
            &self.camera_info
        }

        pub fn tracked_controller_count(&self) -> usize {
            self.controllers.len()
        }

        pub fn update(&mut self) -> Vec<PsmoveApiObservation> {
            unsafe { psmove_tracker_update_image(self.tracker) };
            let mut observations = Vec::new();
            for controller in &self.controllers {
                unsafe { psmove_tracker_update(self.tracker, controller.handle) };
                let mut x = 0.0;
                let mut y = 0.0;
                let mut radius = 0.0;
                let age_ms = unsafe {
                    psmove_tracker_get_position(
                        self.tracker,
                        controller.handle,
                        &mut x,
                        &mut y,
                        &mut radius,
                    )
                };
                if age_ms >= 0 && radius > 0.0 {
                    observations.push(PsmoveApiObservation {
                        move_id: controller.move_id.clone(),
                        center_x_px: x,
                        center_y_px: y,
                        radius_px: radius,
                        age_ms,
                    });
                }
            }
            observations
        }
    }

    impl Drop for PsmoveApiTracker {
        fn drop(&mut self) {
            for controller in self.controllers.drain(..) {
                unsafe {
                    psmove_tracker_disable(self.tracker, controller.handle);
                    psmove_disconnect(controller.handle);
                }
            }
            unsafe { psmove_tracker_free(self.tracker) };
        }
    }

    unsafe fn text(value: *const c_char) -> String {
        if value.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned()
    }
}

#[cfg(target_os = "linux")]
pub use linux::PsmoveApiTracker;

#[cfg(not(target_os = "linux"))]
pub struct PsmoveApiTracker;

#[cfg(not(target_os = "linux"))]
impl PsmoveApiTracker {
    pub fn open(
        _camera_index: i32,
        _exposure: f32,
        _colors: &[(String, [u8; 3])],
    ) -> Result<Self, PsmoveApiTrackerError> {
        Err(PsmoveApiTrackerError(
            "PSMoveAPI tracker backend is currently available on Linux".to_string(),
        ))
    }

    pub fn camera_info(&self) -> &PsmoveApiCameraInfo {
        unreachable!()
    }

    pub fn calibrate_connected(&mut self) -> usize { 0 }

    pub fn observe_connected(&mut self) -> usize { 0 }

    pub fn set_expected_color(&mut self, _move_id: &str, _color: [u8; 3]) -> bool { false }

    pub fn tracked_controller_count(&self) -> usize { 0 }

    pub fn update(&mut self) -> Vec<PsmoveApiObservation> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_preserves_stable_move_identity() {
        let observation = PsmoveApiObservation {
            move_id: "move-000704a39772".to_string(),
            center_x_px: 369.0,
            center_y_px: 430.0,
            radius_px: 10.6,
            age_ms: 1,
        };
        assert_eq!(observation.move_id, "move-000704a39772");
        assert!(observation.radius_px > 0.0);
    }
}
