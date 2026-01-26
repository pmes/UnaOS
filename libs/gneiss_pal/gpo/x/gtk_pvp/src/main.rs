#![allow(deprecated)]
use gtk4::prelude::*;
use libadwaita as adw;
use adw::prelude::*;
use gneiss_pal::{App as CoreApp, Platform, Plugin};
use pvp_core::player::MpvPlayer;
use std::rc::Rc;
use std::cell::RefCell;
use std::any::Any;
use libmpv_sys as mpv;
use once_cell::sync::Lazy;
use std::ffi::{CStr, c_void};
use std::ptr;

// -----------------------------------------------------------------------------
// GL / MPV Helpers
// -----------------------------------------------------------------------------
static GET_GL_INTEGERV: Lazy<extern "C" fn(u32, *mut i32)> = Lazy::new(|| unsafe {
    std::mem::transmute(gl_loader::get_proc_address("glGetIntegerv"))
});

unsafe extern "C" fn get_proc_address(_ctx: *mut c_void, name: *const i8) -> *mut c_void {
    let c_str = unsafe { CStr::from_ptr(name) };
    gl_loader::get_proc_address(c_str.to_str().unwrap()) as *mut _
}

unsafe extern "C" fn on_mpv_render_update(_ctx: *mut c_void) {}

// -----------------------------------------------------------------------------
// GtkPlatform: Exposes the window to plugins
// -----------------------------------------------------------------------------
struct GtkPlatform {
    window: adw::ApplicationWindow,
}

impl Platform for GtkPlatform {
    fn set_title(&self, title: &str) {
        self.window.set_title(Some(title));
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl GtkPlatform {
    pub fn set_content(&self, widget: &impl IsA<gtk4::Widget>) {
        self.window.set_content(Some(widget));
    }
}

// -----------------------------------------------------------------------------
// PvPPlugin: The Video Player Logic
// -----------------------------------------------------------------------------
struct PvPPlugin {
    video_path: String,
    debug_mode: bool,
    player: Option<Rc<RefCell<MpvPlayer>>>,
}

impl PvPPlugin {
    fn new(path: String, debug_mode: bool) -> Self {
        Self {
            video_path: path,
            debug_mode,
            player: None,
        }
    }

    fn setup_ui(&mut self, platform: &GtkPlatform) {
        let player = Rc::new(RefCell::new(MpvPlayer::new(self.debug_mode, "auto".to_string(), "libmpv".to_string())));
        self.player = Some(player.clone());

        let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

        // 1. Video Area
        let gl_area = gtk4::GLArea::new();
        gl_area.set_vexpand(true);
        gl_area.set_hexpand(true);
        main_box.append(&gl_area);

        // 2. Controls
        let controls = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        controls.set_margin_bottom(10);
        controls.set_margin_start(10);
        controls.set_margin_end(10);

        let btn_play = gtk4::Button::from_icon_name("media-playback-start-symbolic");
        let slider = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 100.0, 1.0);
        slider.set_hexpand(true);

        controls.append(&btn_play);
        controls.append(&slider);
        main_box.append(&controls);

        // --- Logic Hooks ---
        let video_path = self.video_path.clone();

        // Init/Realize (MPV GL Context)
        let p_realize = player.clone();
        gl_area.connect_realize(move |area| {
            println!("DEBUG: GL Area Realize called");
            area.make_current();
            let mut p = p_realize.borrow_mut();
            unsafe {
                let mut gl_params = mpv::mpv_opengl_init_params {
                    get_proc_address: Some(get_proc_address),
                    get_proc_address_ctx: ptr::null_mut(),
                    extra_exts: ptr::null(),
                };
                let mut params: [mpv::mpv_render_param; 3] = [
                    mpv::mpv_render_param {
                        type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                        data: mpv::MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut c_void,
                    },
                    mpv::mpv_render_param {
                        type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                        data: &mut gl_params as *mut _ as *mut c_void,
                    },
                    mpv::mpv_render_param {
                        type_: 0,
                        data: ptr::null_mut(),
                    },
                ];
                mpv::mpv_render_context_create(&mut p.render_context, p.handle, params.as_mut_ptr());
                mpv::mpv_render_context_set_update_callback(
                    p.render_context,
                    Some(on_mpv_render_update),
                    ptr::null_mut(),
                );
            }
            p.play_file(&video_path);
        });

        // Render Loop
        let p_render = player.clone();
        gl_area.connect_render(move |area, _| {
             let p = p_render.borrow();
             if !p.render_context.is_null() {
                 unsafe {
                    let mut fbo_id: i32 = 0;
                    GET_GL_INTEGERV(0x8CA6, &mut fbo_id); // GL_DRAW_FRAMEBUFFER_BINDING
                    let mut fbo = mpv::mpv_opengl_fbo {
                        fbo: fbo_id,
                        w: area.allocated_width() * area.scale_factor(),
                        h: area.allocated_height() * area.scale_factor(),
                        internal_format: 0,
                    };
                    let mut params: [mpv::mpv_render_param; 3] = [
                        mpv::mpv_render_param {
                            type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                            data: &mut fbo as *mut _ as *mut c_void,
                        },
                        mpv::mpv_render_param {
                            type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                            data: &mut 1 as *mut i32 as *mut c_void,
                        },
                        mpv::mpv_render_param {
                            type_: 0,
                            data: ptr::null_mut(),
                        },
                    ];
                    mpv::mpv_render_context_render(p.render_context, params.as_mut_ptr());
                 }
             }
             glib::Propagation::Stop
        });

        // Tick Callback (Redraw Trigger)
        let p_tick = player.clone();
        let _last_pos = Rc::new(RefCell::new(-1.0));
        gl_area.add_tick_callback(move |area, _| {
            if let Ok(p) = p_tick.try_borrow() {
                // If rendering context exists, we should ask it if update is needed
                if !p.render_context.is_null() {
                     if unsafe { mpv::mpv_render_context_update(p.render_context) } != 0 {
                         area.queue_draw();
                     }
                }
            }
            glib::ControlFlow::Continue
        });

        // Controls
        let p_btn = player.clone();
        btn_play.connect_clicked(move |_| {
            if let Ok(p) = p_btn.try_borrow() {
                p.toggle_pause();
            }
        });

        // Attach to window LAST to ensure signals are connected before realization
        platform.set_content(&main_box);
    }
}

impl Plugin for PvPPlugin {
    fn on_init(&mut self, platform: &dyn Platform) {
        platform.set_title("PVP Template Test");
        let gtk_platform = platform.as_any().downcast_ref::<GtkPlatform>();
        if let Some(p) = gtk_platform {
            self.setup_ui(p);
        } else {
             eprintln!("Error: Platform is not GtkPlatform!");
        }
    }
    fn on_update(&mut self, _platform: &dyn Platform) {}
    fn as_any(&self) -> &dyn Any { self }
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
}

fn main() {
    let (path_opt, debug_mode) = gneiss_pal::simple_arg_parse();
    let raw_path = path_opt.unwrap_or("test.mp4".to_string());

    // Canonicalize if possible
    let video_path = std::fs::canonicalize(&raw_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| raw_path.clone());

    println!("DEBUG: Attempting to play: {}", video_path);
    if !std::path::Path::new(&video_path).exists() {
        println!("WARNING: File does not exist: {}", video_path);
    }

    gl_loader::init_gl();

    let app = adw::Application::builder()
        .application_id("com.gneiss.demo.gtk")
        .build();

    app.connect_activate(move |app| {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Loading...")
            .default_width(800)
            .default_height(600)
            .build();

        window.present();

        let platform = GtkPlatform { window: window.clone() };
        let mut core_app = CoreApp::new();

        let plugin = PvPPlugin::new(video_path.clone(), debug_mode);
        core_app.register_plugin(plugin);

        core_app.init(&platform);
    });

    app.run_with_args::<&str>(&[]);
}
