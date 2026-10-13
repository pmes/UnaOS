//! The builtin function signatures of GLSL ES §8 (and OES_standard_derivatives' three), resolved by exact
//! argument types (§6.1: no implicit conversions). `genType` is float, vec2, vec3 or vec4.

use super::ast::*;
use alloc::format;
use alloc::string::String;

/// Every builtin function name (a user function may not redeclare one).
pub const NAMES: &[&str] = &[
    "radians", "degrees", "sin", "cos", "tan", "asin", "acos", "atan", "pow", "exp", "log", "exp2", "log2", "sqrt", "inversesqrt", "abs", "sign",
    "floor", "ceil", "fract", "mod", "min", "max", "clamp", "mix", "step", "smoothstep", "length", "distance", "dot", "cross", "normalize",
    "faceforward", "reflect", "refract", "matrixCompMult", "lessThan", "lessThanEqual", "greaterThan", "greaterThanEqual", "equal", "notEqual",
    "any", "all", "not", "texture2D", "texture2DProj", "texture2DLod", "texture2DProjLod", "textureCube", "textureCubeLod", "dFdx", "dFdy",
    "fwidth",
];

fn gent(t: &Ty) -> bool {
    t.arr == 0 && t.base == Base::Float && t.cols == 1 && (1..=4).contains(&t.rows)
}

fn is_f(t: &Ty) -> bool {
    *t == Ty::FLOAT
}

fn bvec(t: &Ty) -> bool {
    t.arr == 0 && t.base == Base::Bool && t.cols == 1 && (2..=4).contains(&t.rows)
}

fn num_vec(t: &Ty) -> bool {
    t.arr == 0 && matches!(t.base, Base::Float | Base::Int) && t.cols == 1 && (2..=4).contains(&t.rows)
}

/// Resolve a call of builtin `name` with argument types `a`. None: not a builtin name. Err: a builtin with
/// no matching overload (or not available in this stage).
pub fn resolve(name: &str, a: &[Ty], fragment: bool, derivatives: bool) -> Option<Result<(Bf, Ty), String>> {
    if !NAMES.contains(&name) {
        return None;
    }
    let no = || Err(format!("'{name}' : no matching overloaded function found"));
    let n = a.len();
    let g1 = |f: Bf| -> Result<(Bf, Ty), String> { if n == 1 && gent(&a[0]) { Ok((f, a[0])) } else { no() } };
    let v4 = Ty::vec(Base::Float, 4);
    let r = match name {
        "radians" => g1(Bf::Radians),
        "degrees" => g1(Bf::Degrees),
        "sin" => g1(Bf::Sin),
        "cos" => g1(Bf::Cos),
        "tan" => g1(Bf::Tan),
        "asin" => g1(Bf::Asin),
        "acos" => g1(Bf::Acos),
        "atan" => {
            if n == 2 && gent(&a[0]) && a[0] == a[1] {
                Ok((Bf::Atan2, a[0]))
            } else {
                g1(Bf::Atan)
            }
        }
        "exp" => g1(Bf::Exp),
        "log" => g1(Bf::Log),
        "exp2" => g1(Bf::Exp2),
        "log2" => g1(Bf::Log2),
        "sqrt" => g1(Bf::Sqrt),
        "inversesqrt" => g1(Bf::InverseSqrt),
        "abs" => g1(Bf::Abs),
        "sign" => g1(Bf::Sign),
        "floor" => g1(Bf::Floor),
        "ceil" => g1(Bf::Ceil),
        "fract" => g1(Bf::Fract),
        "normalize" => g1(Bf::Normalize),
        "pow" => {
            if n == 2 && gent(&a[0]) && a[0] == a[1] { Ok((Bf::Pow, a[0])) } else { no() }
        }
        "mod" | "min" | "max" => {
            let f = match name {
                "mod" => Bf::Mod,
                "min" => Bf::Min,
                _ => Bf::Max,
            };
            if n == 2 && gent(&a[0]) && (a[1] == a[0] || is_f(&a[1])) { Ok((f, a[0])) } else { no() }
        }
        "clamp" => {
            if n == 3 && gent(&a[0]) && ((a[1] == a[0] && a[2] == a[0]) || (is_f(&a[1]) && is_f(&a[2]))) { Ok((Bf::Clamp, a[0])) } else { no() }
        }
        "mix" => {
            if n == 3 && gent(&a[0]) && a[1] == a[0] && (a[2] == a[0] || is_f(&a[2])) { Ok((Bf::Mix, a[0])) } else { no() }
        }
        "step" => {
            if n == 2 && gent(&a[1]) && (a[0] == a[1] || is_f(&a[0])) { Ok((Bf::Step, a[1])) } else { no() }
        }
        "smoothstep" => {
            if n == 3 && gent(&a[2]) && ((a[0] == a[2] && a[1] == a[2]) || (is_f(&a[0]) && is_f(&a[1]))) { Ok((Bf::Smoothstep, a[2])) } else { no() }
        }
        "length" => {
            if n == 1 && gent(&a[0]) { Ok((Bf::Length, Ty::FLOAT)) } else { no() }
        }
        "distance" | "dot" => {
            let f = if name == "dot" { Bf::Dot } else { Bf::Distance };
            if n == 2 && gent(&a[0]) && a[0] == a[1] { Ok((f, Ty::FLOAT)) } else { no() }
        }
        "cross" => {
            let v3 = Ty::vec(Base::Float, 3);
            if n == 2 && a[0] == v3 && a[1] == v3 { Ok((Bf::Cross, v3)) } else { no() }
        }
        "faceforward" => {
            if n == 3 && gent(&a[0]) && a[1] == a[0] && a[2] == a[0] { Ok((Bf::FaceForward, a[0])) } else { no() }
        }
        "reflect" => {
            if n == 2 && gent(&a[0]) && a[1] == a[0] { Ok((Bf::Reflect, a[0])) } else { no() }
        }
        "refract" => {
            if n == 3 && gent(&a[0]) && a[1] == a[0] && is_f(&a[2]) { Ok((Bf::Refract, a[0])) } else { no() }
        }
        "matrixCompMult" => {
            if n == 2 && a[0].is_matrix() && a[0] == a[1] { Ok((Bf::MatrixCompMult, a[0])) } else { no() }
        }
        "lessThan" | "lessThanEqual" | "greaterThan" | "greaterThanEqual" => {
            let f = match name {
                "lessThan" => Bf::LessThan,
                "lessThanEqual" => Bf::LessThanEqual,
                "greaterThan" => Bf::GreaterThan,
                _ => Bf::GreaterThanEqual,
            };
            if n == 2 && num_vec(&a[0]) && a[0] == a[1] { Ok((f, Ty::vec(Base::Bool, a[0].rows))) } else { no() }
        }
        "equal" | "notEqual" => {
            let f = if name == "equal" { Bf::Equal } else { Bf::NotEqual };
            if n == 2 && (num_vec(&a[0]) || bvec(&a[0])) && a[0] == a[1] { Ok((f, Ty::vec(Base::Bool, a[0].rows))) } else { no() }
        }
        "any" | "all" => {
            let f = if name == "any" { Bf::Any } else { Bf::All };
            if n == 1 && bvec(&a[0]) { Ok((f, Ty::BOOL)) } else { no() }
        }
        "not" => {
            if n == 1 && bvec(&a[0]) { Ok((Bf::Not, a[0])) } else { no() }
        }
        "texture2D" | "texture2DProj" | "texture2DLod" | "texture2DProjLod" | "textureCube" | "textureCubeLod" => {
            let lod = name.ends_with("Lod");
            let cube = name.starts_with("textureCube");
            let proj = name.starts_with("texture2DProj");
            let samp = if cube { Base::SamplerCube } else { Base::Sampler2D };
            if n < 2 || a[0].base != samp || a[0].arr != 0 {
                return Some(no());
            }
            let kind = match (cube, proj, a[1]) {
                (true, _, t) if t == Ty::vec(Base::Float, 3) => TexKind::Cube,
                (false, false, t) if t == Ty::vec(Base::Float, 2) => TexKind::T2D,
                (false, true, t) if t == Ty::vec(Base::Float, 3) => TexKind::T2DProj3,
                (false, true, t) if t == Ty::vec(Base::Float, 4) => TexKind::T2DProj4,
                _ => return Some(no()),
            };
            let extra = match n {
                2 => false,
                3 if is_f(&a[2]) => true,
                _ => return Some(no()),
            };
            if lod {
                // The Lod forms are vertex-shader only (§8.7) and need the lod argument.
                if fragment {
                    return Some(Err(format!("'{name}' : no matching overloaded function found (vertex shader only)")));
                }
                if !extra {
                    return Some(no());
                }
            } else if extra && !fragment {
                // The bias forms are fragment-shader only (§8.7).
                return Some(Err(format!("'{name}' : the bias parameter is only available in fragment shaders")));
            }
            Ok((Bf::Texture(kind, extra, lod), v4))
        }
        "dFdx" | "dFdy" | "fwidth" => {
            if !fragment || !derivatives {
                return Some(Err(format!("'{name}' : requires #extension GL_OES_standard_derivatives in a fragment shader")));
            }
            let f = match name {
                "dFdx" => Bf::DFdx,
                "dFdy" => Bf::DFdy,
                _ => Bf::Fwidth,
            };
            g1(f)
        }
        _ => no(),
    };
    Some(r)
}
