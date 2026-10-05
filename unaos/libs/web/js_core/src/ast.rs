//! The abstract syntax tree. Node kinds follow ESTree (Program, VariableDeclaration, ArrowFunctionExpression,
//! ChainExpression, …) expressed as Rust enums; every scope-bearing node carries a `ScopeId` assigned by the
//! parser so later passes (scope analysis, code generation) can attach data without re-walking.

use crate::lexer::Atom;
use crate::string::JsStr;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

pub type ScopeId = u32;

#[derive(Clone, Debug)]
pub struct Ident {
    pub name: Atom,
    pub span: Span,
}

#[derive(Debug)]
pub struct Program {
    pub body: Vec<Stmt>,
    pub module: bool,
    pub strict: bool,
    pub scope: ScopeId,
    pub scope_count: u32,
    pub source: Rc<[u16]>,
    /// The program contains a direct `eval` call somewhere outside nested non-arrow functions.
    pub has_top_await: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarKind {
    Var,
    Let,
    Const,
}

#[derive(Debug)]
pub struct VarDecl {
    pub kind: VarKind,
    pub decls: Vec<Declarator>,
    pub span: Span,
}

#[derive(Debug)]
pub struct Declarator {
    pub target: Pat,
    pub init: Option<Expr>,
}

#[derive(Debug)]
pub enum Stmt {
    Expr(Box<Expr>, Span),
    Var(Box<VarDecl>),
    Function(Rc<Function>),
    Class(Rc<Class>),
    Return(Option<Box<Expr>>, Span),
    If(Box<Expr>, Box<Stmt>, Option<Box<Stmt>>, Span),
    Block(Box<Block>),
    Empty(Span),
    Debugger(Span),
    For(Box<ForStmt>),
    ForIn(Box<ForInStmt>),
    ForOf(Box<ForInStmt>),
    While(Box<Expr>, Box<Stmt>, Span),
    DoWhile(Box<Stmt>, Box<Expr>, Span),
    Break(Option<Atom>, Span),
    Continue(Option<Atom>, Span),
    Throw(Box<Expr>, Span),
    Try(Box<TryStmt>),
    Switch(Box<SwitchStmt>),
    Labeled(Atom, Box<Stmt>, Span),
    With(Box<Expr>, Box<Stmt>, ScopeId, Span),
    Import(Box<ImportDecl>),
    Export(Box<ExportDecl>),
}

#[derive(Debug)]
pub struct Block {
    pub body: Vec<Stmt>,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Debug)]
pub enum ForInit {
    Var(Box<VarDecl>),
    Expr(Box<Expr>),
}

#[derive(Debug)]
pub struct ForStmt {
    pub init: Option<ForInit>,
    pub test: Option<Expr>,
    pub update: Option<Expr>,
    pub body: Stmt,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Debug)]
pub enum ForHead {
    /// `var x` / `let x` / `const [a, b]` (the declaration's binding pattern).
    Decl(VarKind, Pat),
    /// `var x = init in …` (Annex B.3.5 initialiser in for-in, sloppy only).
    VarInit(Pat, Box<Expr>),
    /// A LeftHandSideExpression / assignment pattern.
    Target(Pat),
}

#[derive(Debug)]
pub struct ForInStmt {
    pub left: ForHead,
    pub right: Expr,
    pub body: Stmt,
    pub is_await: bool,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Debug)]
pub struct Catch {
    pub param: Option<Pat>,
    pub body: Block,
    pub scope: ScopeId,
}

#[derive(Debug)]
pub struct TryStmt {
    pub block: Block,
    pub handler: Option<Catch>,
    pub finalizer: Option<Block>,
    pub span: Span,
}

#[derive(Debug)]
pub struct SwitchCase {
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug)]
pub struct SwitchStmt {
    pub disc: Expr,
    pub cases: Vec<SwitchCase>,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ImportSpec {
    Default(Ident),
    Namespace(Ident),
    Named(JsStr, Ident),
}

#[derive(Debug)]
pub struct ImportDecl {
    pub specs: Vec<ImportSpec>,
    pub source: JsStr,
    pub attributes: Vec<(JsStr, JsStr)>,
    pub span: Span,
}

#[derive(Debug)]
pub enum ExportDecl {
    /// `export var/let/const/function/class …`
    Decl(Stmt),
    /// `export { a as b, … } [from "m"]`; (local-or-imported, exported)
    Named { specs: Vec<(JsStr, JsStr)>, source: Option<JsStr>, attributes: Vec<(JsStr, JsStr)>, span: Span },
    /// `export * [as ns] from "m"`
    All { exported: Option<JsStr>, source: JsStr, attributes: Vec<(JsStr, JsStr)>, span: Span },
    DefaultExpr(Expr, Span),
    DefaultFunction(Rc<Function>),
    DefaultClass(Rc<Class>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Minus,
    Plus,
    Not,
    BitNot,
    Typeof,
    Void,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
    UShr,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    BitOr,
    BitXor,
    BitAnd,
    In,
    InstanceOf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicalOp {
    And,
    Or,
    Nullish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Bin(BinOp),
    Logical(LogicalOp),
}

#[derive(Debug)]
pub enum MemberProp {
    Name(Atom),
    Computed(Expr),
    Private(Atom),
}

#[derive(Debug)]
pub enum Arg {
    Expr(Expr),
    Spread(Expr),
}

#[derive(Debug)]
pub enum ArrayElem {
    Hole,
    Expr(Expr),
    Spread(Expr),
}

#[derive(Debug, Clone)]
pub enum PropKey {
    Name(Atom),
    Str(JsStr),
    Num(f64),
    BigInt(Atom),
    Computed(Rc<Expr>),
    Private(Atom),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    Method,
    Get,
    Set,
}

#[derive(Debug)]
pub enum Prop {
    KeyValue(PropKey, Expr),
    Shorthand(Ident),
    Method(PropKey, Rc<Function>, MethodKind),
    Spread(Expr),
    /// `__proto__: value` (non-computed, non-shorthand): sets [[Prototype]].
    Proto(Expr, Span),
    /// CoverInitializedName `{ a = 1 }`, valid only as an assignment pattern.
    CoverInit(Ident, Expr),
}

#[derive(Debug)]
pub struct TemplateLit {
    pub quasis: Vec<(Option<JsStr>, JsStr)>,
    pub exprs: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug)]
pub enum Expr {
    Ident(Ident),
    This(Span),
    Num(f64, Span),
    Str(JsStr, Span),
    BigInt(Atom, Span),
    Bool(bool, Span),
    Null(Span),
    Regex(JsStr, JsStr, Span),
    Template(Box<TemplateLit>),
    /// Tagged template; `site` is a unique id for the template object cache.
    TaggedTemplate(Box<Expr>, Rc<TemplateLit>, u32, Span),
    Array(Vec<ArrayElem>, Span),
    Object(Vec<Prop>, Span),
    Function(Rc<Function>),
    Arrow(Rc<Function>),
    Class(Rc<Class>),
    Unary(UnaryOp, Box<Expr>, Span),
    Update(bool /*inc*/, bool /*prefix*/, Box<Expr>, Span),
    Binary(BinOp, Box<Expr>, Box<Expr>, Span),
    Logical(LogicalOp, Box<Expr>, Box<Expr>, Span),
    Assign(AssignOp, Box<Pat>, Box<Expr>, Span),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>, Span),
    Call(Box<Expr>, Vec<Arg>, bool /*optional*/, Span),
    New(Box<Expr>, Vec<Arg>, Span),
    Member(Box<Expr>, Box<MemberProp>, bool /*optional*/, Span),
    SuperMember(Box<MemberProp>, Span),
    SuperCall(Vec<Arg>, Span),
    /// ChainExpression: the boundary of an optional chain.
    Chain(Box<Expr>, Span),
    Seq(Vec<Expr>, Span),
    Yield(Option<Box<Expr>>, bool /*delegate*/, Span),
    Await(Box<Expr>, Span),
    NewTarget(Span),
    ImportMeta(Span),
    ImportCall(Box<Expr>, Option<Box<Expr>>, Span),
    PrivateIn(Atom, Box<Expr>, Span),
    Paren(Box<Expr>, Span),
    /// Parser-internal: arrow parameter cover `()` / `(a, ...b)` / `(a,)` (never reaches later passes).
    CoverParams(Vec<Expr>, Option<Box<Expr>>, Span),
    /// Parser-internal: a rest binding pattern inside an arrow parameter cover.
    CoverPat(Box<Pat>),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Ident(i) => i.span,
            Expr::This(s) | Expr::Num(_, s) | Expr::Str(_, s) | Expr::BigInt(_, s) | Expr::Bool(_, s) | Expr::Null(s) => *s,
            Expr::Regex(_, _, s) => *s,
            Expr::Template(t) => t.span,
            Expr::TaggedTemplate(_, _, _, s) => *s,
            Expr::Array(_, s) | Expr::Object(_, s) => *s,
            Expr::Function(f) | Expr::Arrow(f) => f.span,
            Expr::Class(c) => c.span,
            Expr::Unary(_, _, s) | Expr::Update(_, _, _, s) | Expr::Binary(_, _, _, s) | Expr::Logical(_, _, _, s) => *s,
            Expr::Assign(_, _, _, s) | Expr::Cond(_, _, _, s) | Expr::Call(_, _, _, s) | Expr::New(_, _, s) => *s,
            Expr::Member(_, _, _, s) | Expr::SuperMember(_, s) | Expr::SuperCall(_, s) | Expr::Chain(_, s) => *s,
            Expr::Seq(_, s) | Expr::Yield(_, _, s) | Expr::Await(_, s) | Expr::NewTarget(s) | Expr::ImportMeta(s) => *s,
            Expr::ImportCall(_, _, s) | Expr::PrivateIn(_, _, s) | Expr::Paren(_, s) => *s,
            Expr::CoverParams(_, _, s) => *s,
            Expr::CoverPat(p) => p.span(),
        }
    }
    /// Strip parentheses.
    pub fn unparen(&self) -> &Expr {
        let mut e = self;
        while let Expr::Paren(inner, _) = e {
            e = inner;
        }
        e
    }
    /// IsAnonymousFunctionDefinition (§8.4.3).
    pub fn is_anonymous_fn(&self) -> bool {
        match self.unparen() {
            Expr::Function(f) => f.id.is_none(),
            Expr::Arrow(_) => true,
            Expr::Class(c) => c.id.is_none(),
            _ => false,
        }
    }
}

#[derive(Debug)]
pub struct PatProp {
    pub key: PropKey,
    pub value: Pat,
}

#[derive(Debug)]
pub enum Pat {
    Ident(Ident),
    /// A member-expression target (assignment patterns only).
    Expr(Box<Expr>),
    Object(Vec<PatProp>, Option<Box<Pat>>, Span),
    Array(Vec<Option<Pat>>, Option<Box<Pat>>, Span),
    /// A target with a default value.
    Assign(Box<Pat>, Box<Expr>, Span),
}

impl Pat {
    pub fn span(&self) -> Span {
        match self {
            Pat::Ident(i) => i.span,
            Pat::Expr(e) => e.span(),
            Pat::Object(_, _, s) | Pat::Array(_, _, s) | Pat::Assign(_, _, s) => *s,
        }
    }
    /// BoundNames (§8.2.1).
    pub fn bound_names(&self, out: &mut Vec<Ident>) {
        match self {
            Pat::Ident(i) => out.push(i.clone()),
            Pat::Expr(_) => {}
            Pat::Object(props, rest, _) => {
                for p in props {
                    p.value.bound_names(out);
                }
                if let Some(r) = rest {
                    r.bound_names(out);
                }
            }
            Pat::Array(elems, rest, _) => {
                for p in elems.iter().flatten() {
                    p.bound_names(out);
                }
                if let Some(r) = rest {
                    r.bound_names(out);
                }
            }
            Pat::Assign(p, _, _) => p.bound_names(out),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnKind {
    Normal,
    Arrow,
    Method,
    Getter,
    Setter,
    ClassConstructor,
    /// A class field initialiser (wrapped as a method so `this`/`super` work).
    FieldInit,
    StaticBlock,
}

#[derive(Debug)]
pub struct Function {
    pub id: Option<Ident>,
    pub params: Vec<Pat>,
    pub rest: Option<Pat>,
    pub body: Vec<Stmt>,
    /// Concise arrow body / field initialiser expression.
    pub expr_body: Option<Box<Expr>>,
    pub kind: FnKind,
    pub is_async: bool,
    pub is_generator: bool,
    pub strict: bool,
    pub simple_params: bool,
    pub derived: bool,
    /// Source range for Function.prototype.toString.
    pub span: Span,
    pub scope: ScopeId,
    /// Scope of the body's lexical declarations (distinct from `scope` for parameter expressions / sloppy).
    pub body_scope: ScopeId,
    /// ExpectedArgumentCount.
    pub length: u32,
    /// The function (or an arrow nested in it) contains a direct `eval` call.
    pub has_direct_eval: bool,
    /// Function (incl. nested arrows / eval) references `arguments`.
    pub uses_arguments: bool,
    pub uses_this: bool,
    pub uses_super: bool,
    /// For class constructors: the class's fields need initialising.
    pub class_fields: bool,
    /// Function expressions with a name bind it in a scope of their own.
    pub name_scope: ScopeId,
}

#[derive(Debug)]
pub enum ClassMember {
    Method { key: PropKey, func: Rc<Function>, kind: MethodKind, is_static: bool },
    Field { key: PropKey, init: Option<Rc<Function>>, is_static: bool, span: Span },
    StaticBlock(Rc<Function>),
}

#[derive(Debug)]
pub struct Class {
    pub id: Option<Ident>,
    pub super_class: Option<Box<Expr>>,
    pub constructor: Option<Rc<Function>>,
    pub members: Vec<ClassMember>,
    pub scope: ScopeId,
    pub span: Span,
}
