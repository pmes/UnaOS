//! Shader and program objects' entry points (ES 2.0 §2.10.1–§2.10.3, WebGL 1.0 shaderSource / compileShader /
//! getShaderParameter / getShaderInfoLog / getShaderSource).

use crate::consts::*;
use crate::context::{Context, Param};
use crate::glsl;
use alloc::rc::Rc;
use alloc::string::String;

impl Context {
    pub fn shader_source(&mut self, shader: u32, src: &str) {
        let Some(s) = self.shaders.get_mut(shader) else { return self.err(INVALID_VALUE) };
        s.source = src.into();
    }

    /// ES 2.0 §2.10.1 CompileShader: COMPILE_STATUS and the info log are replaced; a previously compiled
    /// unit stays attached to programs already linked (they keep their own copy).
    pub fn compile_shader(&mut self, shader: u32) {
        let derivs = self.ext.standard_derivatives;
        let Some(s) = self.shaders.get_mut(shader) else { return self.err(INVALID_VALUE) };
        let (unit, log) = glsl::compile(&s.source, s.kind == FRAGMENT_SHADER, derivs);
        s.compiled = unit.is_some();
        s.code = unit.map(Rc::new);
        s.log = log;
    }

    pub fn get_shader_parameter(&mut self, shader: u32, pname: u32) -> Param {
        let Some(s) = self.shaders.get(shader) else {
            self.err(INVALID_VALUE);
            return Param::Null;
        };
        match pname {
            SHADER_TYPE => Param::Int(s.kind as i64),
            DELETE_STATUS => Param::Bool(s.deleted),
            COMPILE_STATUS => Param::Bool(s.compiled),
            _ => {
                self.err(INVALID_ENUM);
                Param::Null
            }
        }
    }

    pub fn get_shader_info_log(&mut self, shader: u32) -> Option<String> {
        match self.shaders.get(shader) {
            Some(s) => Some(s.log.clone()),
            None => {
                self.err(INVALID_VALUE);
                None
            }
        }
    }

    pub fn get_shader_source(&mut self, shader: u32) -> Option<String> {
        match self.shaders.get(shader) {
            Some(s) => Some(s.source.clone()),
            None => {
                self.err(INVALID_VALUE);
                None
            }
        }
    }
}
