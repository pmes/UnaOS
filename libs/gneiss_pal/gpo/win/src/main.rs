#[cfg(windows)]
mod win_impl {
    use gneiss_pal::{App as CoreApp, Platform, Plugin};
    use windows::{
        core::*,
        Win32::Foundation::*,
        Win32::System::LibraryLoader::GetModuleHandleW,
        Win32::UI::WindowsAndMessaging::*,
    };

    pub struct WinPlatform {
        hwnd: HWND,
    }

    impl Platform for WinPlatform {
        fn set_title(&self, title: &str) {
            unsafe {
                let wide_title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = SetWindowTextW(self.hwnd, PCWSTR(wide_title.as_ptr()));
            }
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct HelloPlugin;
    impl Plugin for HelloPlugin {
        fn on_init(&mut self, platform: &dyn Platform) {
            platform.set_title("Hello from Windows Skeleton");
            println!("Windows Plugin Initialized!");
        }
        fn on_update(&mut self, _platform: &dyn Platform) {}
        fn as_any(&self) -> &dyn std::any::Any { self }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    }

    pub fn main() -> Result<()> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class_name = w!("TemplateAppClass");

            let wc = WNDCLASSW {
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hInstance: instance.into(),
                lpszClassName: class_name,
                lpfnWndProc: Some(wnd_proc),
                ..Default::default()
            };

            RegisterClassW(&wc);

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("Template App"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                800,
                600,
                None,
                None,
                instance,
                None,
            );

            let platform = WinPlatform { hwnd };
            let mut app = CoreApp::new();
            app.register_plugin(HelloPlugin);
            app.init(&platform);

            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).into() {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }

    extern "system" fn wnd_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
            match message {
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(window, message, wparam, lparam),
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    win_impl::main().unwrap();
    #[cfg(not(windows))]
    println!("Windows Template requires Windows to run. (Check passed on Linux)");
}
