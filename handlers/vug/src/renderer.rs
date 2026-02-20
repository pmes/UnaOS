use epoxy;
use euclase::mat::Mat4;
use euclase::vec::{Vec3, Vec4};
use gtk4::prelude::*;
use gtk4::{gdk::GLContext, GLArea};
use std::ffi::CString;
use std::mem;
use std::ptr;
use std::time::Instant;

const VERTEX_SHADER_SRC: &str = r#"
#version 330 core
layout (location = 0) in vec3 aPos;
layout (location = 1) in vec3 aColor;

out vec3 ourColor;

uniform mat4 model;
uniform mat4 view;
uniform mat4 projection;

void main() {
    gl_Position = projection * view * model * vec4(aPos, 1.0);
    ourColor = aColor;
}
"#;

const FRAGMENT_SHADER_SRC: &str = r#"
#version 330 core
in vec3 ourColor;
out vec4 FragColor;

void main() {
    FragColor = vec4(ourColor, 1.0);
}
"#;

pub struct Renderer {
    program: u32,
    vao: u32,
    vbo: u32,
    ebo: u32,
    start_time: Instant,
    spectrum: Vec<f32>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            program: 0,
            vao: 0,
            vbo: 0,
            ebo: 0,
            start_time: Instant::now(),
            spectrum: Vec::new(),
        }
    }

    pub fn update_spectrum(&mut self, data: Vec<f32>) {
        self.spectrum = data;
    }

    fn init(&mut self) {
        gl::load_with(|s| epoxy::get_proc_addr(s));

        unsafe {
            let vertex_shader = compile_shader(gl::VERTEX_SHADER, VERTEX_SHADER_SRC);
            let fragment_shader = compile_shader(gl::FRAGMENT_SHADER, FRAGMENT_SHADER_SRC);
            self.program = link_program(vertex_shader, fragment_shader);
            gl::DeleteShader(vertex_shader);
            gl::DeleteShader(fragment_shader);

            // Cube Vertices (Pos + Color)
            let vertices: [f32; 48] = [
                // Front
                -0.5, -0.5, 0.5,  1.0, 0.0, 0.0,
                 0.5, -0.5, 0.5,  0.0, 1.0, 0.0,
                 0.5,  0.5, 0.5,  0.0, 0.0, 1.0,
                -0.5,  0.5, 0.5,  1.0, 1.0, 0.0,
                // Back
                -0.5, -0.5, -0.5, 0.0, 1.0, 1.0,
                 0.5, -0.5, -0.5, 1.0, 0.0, 1.0,
                 0.5,  0.5, -0.5, 1.0, 1.0, 1.0,
                -0.5,  0.5, -0.5, 0.5, 0.5, 0.5,
            ];

            let indices: [u32; 36] = [
                0, 1, 2, 2, 3, 0, // Front
                1, 5, 6, 6, 2, 1, // Right
                5, 4, 7, 7, 6, 5, // Back
                4, 0, 3, 3, 7, 4, // Left
                3, 2, 6, 6, 7, 3, // Top
                4, 5, 1, 1, 0, 4, // Bottom
            ];

            gl::GenVertexArrays(1, &mut self.vao);
            gl::GenBuffers(1, &mut self.vbo);
            gl::GenBuffers(1, &mut self.ebo);

            gl::BindVertexArray(self.vao);

            gl::BindBuffer(gl::ARRAY_BUFFER, self.vbo);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                (vertices.len() * mem::size_of::<f32>()) as isize,
                vertices.as_ptr() as *const _,
                gl::STATIC_DRAW,
            );

            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, self.ebo);
            gl::BufferData(
                gl::ELEMENT_ARRAY_BUFFER,
                (indices.len() * mem::size_of::<u32>()) as isize,
                indices.as_ptr() as *const _,
                gl::STATIC_DRAW,
            );

            // Pos
            gl::VertexAttribPointer(
                0,
                3,
                gl::FLOAT,
                gl::FALSE,
                (6 * mem::size_of::<f32>()) as i32,
                ptr::null(),
            );
            gl::EnableVertexAttribArray(0);

            // Color
            gl::VertexAttribPointer(
                1,
                3,
                gl::FLOAT,
                gl::FALSE,
                (6 * mem::size_of::<f32>()) as i32,
                (3 * mem::size_of::<f32>()) as *const _,
            );
            gl::EnableVertexAttribArray(1);

            gl::BindBuffer(gl::ARRAY_BUFFER, 0);
            gl::BindVertexArray(0);
        }
    }

    pub fn draw(&mut self, area: &GLArea, _ctx: &GLContext) -> glib::Propagation {
        if self.program == 0 {
            self.init();
        }

        unsafe {
            gl::ClearColor(0.1, 0.1, 0.1, 1.0);
            gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
            gl::Enable(gl::DEPTH_TEST);

            gl::UseProgram(self.program);

            let now = self.start_time.elapsed().as_secs_f32();
            let angle = now * 1.0;

            let scale_val = if self.spectrum.is_empty() {
                1.0
            } else {
                1.0 + self.spectrum.iter().sum::<f32>() / self.spectrum.len() as f32 * 10.0
            };

            let scale = Mat4::from_scale(Vec3::new(scale_val, scale_val, scale_val));
            // Manual rotation multiplication since * might not be implemented for Mat4 directly in Euclase
            // or to be safe. Actually Euclase implements Mul for Mat4.
            // But let's construct the model matrix carefully.
            // model = Ry * Rx * Scale
            let rx = rotate_x(angle * 0.5);
            let ry = rotate_y(angle);
            let model = ry * rx * scale;

            let view = Mat4::look_at_rh(
                Vec3::new(0.0, 2.0, 4.0),
                Vec3::ZERO,
                Vec3::Y,
            );
            let projection = Mat4::perspective_rh_gl(
                45.0f32.to_radians(),
                area.width() as f32 / area.height() as f32,
                0.1,
                100.0,
            );

            let model_loc = CString::new("model").unwrap();
            let view_loc = CString::new("view").unwrap();
            let proj_loc = CString::new("projection").unwrap();

            gl::UniformMatrix4fv(
                gl::GetUniformLocation(self.program, model_loc.as_ptr()),
                1,
                gl::FALSE,
                model.to_cols_array().as_ptr(),
            );
            gl::UniformMatrix4fv(
                gl::GetUniformLocation(self.program, view_loc.as_ptr()),
                1,
                gl::FALSE,
                view.to_cols_array().as_ptr(),
            );
            gl::UniformMatrix4fv(
                gl::GetUniformLocation(self.program, proj_loc.as_ptr()),
                1,
                gl::FALSE,
                projection.to_cols_array().as_ptr(),
            );

            gl::BindVertexArray(self.vao);
            gl::DrawElements(gl::TRIANGLES, 36, gl::UNSIGNED_INT, ptr::null());
        }

        area.queue_render(); // Request continuous redraw for animation
        glib::Propagation::Proceed
    }
}

unsafe fn compile_shader(shader_type: u32, source: &str) -> u32 {
    let shader = gl::CreateShader(shader_type);
    let c_str = CString::new(source.as_bytes()).unwrap();
    gl::ShaderSource(shader, 1, &c_str.as_ptr(), ptr::null());
    gl::CompileShader(shader);
    shader
}

unsafe fn link_program(vertex: u32, fragment: u32) -> u32 {
    let program = gl::CreateProgram();
    gl::AttachShader(program, vertex);
    gl::AttachShader(program, fragment);
    gl::LinkProgram(program);
    program
}

fn rotate_y(angle: f32) -> Mat4 {
    let s = angle.sin();
    let c = angle.cos();
    let col0 = Vec4::new(c, 0.0, -s, 0.0);
    let col1 = Vec4::new(0.0, 1.0, 0.0, 0.0);
    let col2 = Vec4::new(s, 0.0, c, 0.0);
    let col3 = Vec4::new(0.0, 0.0, 0.0, 1.0);
    Mat4::from_cols(col0, col1, col2, col3)
}

fn rotate_x(angle: f32) -> Mat4 {
    let s = angle.sin();
    let c = angle.cos();
    let col0 = Vec4::new(1.0, 0.0, 0.0, 0.0);
    let col1 = Vec4::new(0.0, c, s, 0.0);
    let col2 = Vec4::new(0.0, -s, c, 0.0);
    let col3 = Vec4::new(0.0, 0.0, 0.0, 1.0);
    Mat4::from_cols(col0, col1, col2, col3)
}
