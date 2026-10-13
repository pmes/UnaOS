//! The GLSL ES 1.00 abstract syntax tree (the §9 grammar's shapes) and its type model (§4.1).
//!
//! The parser builds it; [`super::sema`] resolves names in place (`Name` → `Var`, `Call` → `UserCall` /
//! `Builtin` / `Ctor`, `Field` → `Member` / `Swizzle`) and stamps every expression's type, so the code
//! generator reads a fully typed tree.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Void,
    Float,
    Int,
    Bool,
    Sampler2D,
    SamplerCube,
    Struct(u16),
}

/// A GLSL type: `rows` = vector size (1 for scalars) or matrix rows, `cols` = matrix columns (1 otherwise),
/// `arr` = array length (0 = not an array; GLSL ES 1.00 arrays are one-dimensional, §4.1.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ty {
    pub base: Base,
    pub rows: u8,
    pub cols: u8,
    pub arr: u16,
}

impl Ty {
    pub const VOID: Ty = Ty { base: Base::Void, rows: 1, cols: 1, arr: 0 };
    pub const FLOAT: Ty = Ty { base: Base::Float, rows: 1, cols: 1, arr: 0 };
    pub const INT: Ty = Ty { base: Base::Int, rows: 1, cols: 1, arr: 0 };
    pub const BOOL: Ty = Ty { base: Base::Bool, rows: 1, cols: 1, arr: 0 };
    /// An invalid type (an expression that already reported an error).
    pub const ERR: Ty = Ty { base: Base::Void, rows: 0, cols: 0, arr: 0 };

    pub fn vec(base: Base, n: u8) -> Ty {
        Ty { base, rows: n, cols: 1, arr: 0 }
    }
    pub fn mat(n: u8) -> Ty {
        Ty { base: Base::Float, rows: n, cols: n, arr: 0 }
    }
    pub fn is_err(&self) -> bool {
        self.rows == 0
    }
    pub fn elem(&self) -> Ty {
        Ty { arr: 0, ..*self }
    }
    pub fn is_array(&self) -> bool {
        self.arr > 0
    }
    pub fn is_scalar(&self) -> bool {
        self.arr == 0 && self.rows == 1 && self.cols == 1 && matches!(self.base, Base::Float | Base::Int | Base::Bool)
    }
    pub fn is_vector(&self) -> bool {
        self.arr == 0 && self.rows > 1 && self.cols == 1 && matches!(self.base, Base::Float | Base::Int | Base::Bool)
    }
    pub fn is_matrix(&self) -> bool {
        self.arr == 0 && self.cols > 1
    }
    pub fn is_numeric(&self) -> bool {
        self.arr == 0 && matches!(self.base, Base::Float | Base::Int)
    }
    pub fn is_sampler(&self) -> bool {
        matches!(self.base, Base::Sampler2D | Base::SamplerCube)
    }
    pub fn components(&self) -> u32 {
        self.rows as u32 * self.cols as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prec {
    None,
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    And,
    Or,
    Xor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Pos,
    Not,
    PreInc,
    PreDec,
    PostInc,
    PostDec,
}

/// Assignment operators (§5.8; `%=`, `<<=`, `>>=`, `&=`, `^=`, `|=` are reserved and rejected by the parser).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsgOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
}

/// The builtin functions of GLSL ES §8 (one variant per distinct operation; overloads share a variant and
/// differ in their stamped argument types).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bf {
    Radians,
    Degrees,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Pow,
    Exp,
    Log,
    Exp2,
    Log2,
    Sqrt,
    InverseSqrt,
    Abs,
    Sign,
    Floor,
    Ceil,
    Fract,
    Mod,
    Min,
    Max,
    Clamp,
    Mix,
    Step,
    Smoothstep,
    Length,
    Distance,
    Dot,
    Cross,
    Normalize,
    FaceForward,
    Reflect,
    Refract,
    MatrixCompMult,
    LessThan,
    LessThanEqual,
    GreaterThan,
    GreaterThanEqual,
    Equal,
    NotEqual,
    Any,
    All,
    Not,
    /// texture2D / texture2DProj (vec3 or vec4 coordinate) / textureCube, with or without bias, and the
    /// *Lod forms; the variant carries (kind, has bias-or-lod operand, is explicit lod).
    Texture(TexKind, bool, bool),
    DFdx,
    DFdy,
    Fwidth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexKind {
    T2D,
    T2DProj3,
    T2DProj4,
    Cube,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub k: EK,
    pub line: u32,
    pub ty: Ty,
}

#[derive(Clone, Debug)]
pub enum EK {
    Int(i64),
    Float(f32),
    Bool(bool),
    Name(String),
    Var(u32),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Un(UnOp, Box<Expr>),
    Assign(AsgOp, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Comma(Box<Expr>, Box<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Field(Box<Expr>, String),
    Member(Box<Expr>, u16),
    /// Swizzle: the selected lanes (`sel[0..n]`).
    Swizzle(Box<Expr>, [u8; 4], u8),
    /// A call by name (function, or a struct constructor).
    Call(String, Vec<Expr>),
    /// A constructor of a builtin or struct type.
    Ctor(Ty, Vec<Expr>),
    UserCall(u32, Vec<Expr>),
    Builtin(Bf, Vec<Expr>),
}

impl Expr {
    pub fn new(k: EK, line: u32) -> Expr {
        Expr { k, line, ty: Ty::ERR }
    }
}

#[derive(Clone, Debug)]
pub struct VarDecl {
    pub name: String,
    pub ty: Ty,
    /// An array size expression still to be evaluated (`float a[N]`), or `[]` (Some(None) — an error in 1.00).
    pub size: Option<Option<Expr>>,
    pub init: Option<Expr>,
    pub line: u32,
    /// The symbol id, set by sema.
    pub id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Qual {
    None,
    Const,
    Attribute,
    Uniform,
    Varying,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamQual {
    In,
    Out,
    InOut,
}

#[derive(Clone, Debug)]
pub struct DeclGroup {
    pub qual: Qual,
    pub invariant: bool,
    pub prec: Prec,
    pub decls: Vec<VarDecl>,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Decl(DeclGroup),
    Expr(Expr),
    Block(Vec<Stmt>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    /// for (init; cond; step) body — cond may be a declaration (`bool b = …`), carried as a Decl in `cond_decl`.
    For(Box<Stmt>, Option<Expr>, Option<DeclGroup>, Option<Expr>, Box<Stmt>),
    While(Option<Expr>, Option<DeclGroup>, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    Return(Option<Expr>, u32),
    Break(u32),
    Continue(u32),
    Discard(u32),
    /// A precision statement inside a block (§4.5.3: they are scoped).
    Precision(Prec, Ty, u32),
    /// A struct definition point (its field array sizes are evaluated here, in scope order).
    Struct(u16),
    Empty,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: Option<String>,
    pub ty: Ty,
    pub size: Option<Option<Expr>>,
    pub qual: ParamQual,
    pub cnst: bool,
    pub prec: Prec,
    pub line: u32,
    /// The parameter's symbol id, set by sema.
    pub id: u32,
}

#[derive(Clone, Debug)]
pub struct FuncDef {
    pub name: String,
    pub ret: Ty,
    pub ret_prec: Prec,
    pub params: Vec<Param>,
    pub body: Option<Vec<Stmt>>,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Ty, Prec)>,
    /// Field array sizes still to evaluate (index-aligned with `fields`).
    pub sizes: Vec<Option<Option<Expr>>>,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum Ext {
    /// `precision mediump float;` (§4.5.3)
    Precision(Prec, Ty, u32),
    /// `invariant gl_Position, v;` (§4.6.1)
    Invariant(Vec<String>, u32),
    Decl(DeclGroup),
    Func(FuncDef),
    Struct(u16),
}

pub struct Unit {
    pub items: Vec<Ext>,
    pub structs: Vec<StructDef>,
}
