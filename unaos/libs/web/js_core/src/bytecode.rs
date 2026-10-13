//! The bytecode: a stack machine. Each function compiles to a `Code` with an instruction vector, a constant
//! pool and a register file (locals) below the operand stack.

use crate::ast::FnKind;
use crate::string::JsStr;
use crate::vm::value::BigInt;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    // ---- constants / stack
    Undef,
    Null,
    True,
    False,
    Int(i32),
    Const(u32),
    PushEmpty,
    Pop,
    Dup,
    /// a b -> a b a b
    Dup2,
    Swap,
    /// a b -> a b a
    Over,
    /// a b c -> c a b (move top under the next two)
    Rot3,
    /// a b c d -> d a b c
    Rot4,
    /// a b c -> b c a (bring the third to top)
    Rot3Up,

    // ---- bindings
    GetLocal(u32),
    SetLocal(u32),
    PutLocal(u32),
    GetLocalChk(u32, u32),
    SetLocalChk(u32, u32),
    GetEnv(u16, u32),
    GetEnvChk(u16, u32),
    SetEnv(u16, u32),
    SetEnvChk(u16, u32),
    InitEnv(u16, u32),
    /// TypeError: assignment to a constant (operand: name const).
    ThrowConst(u32),
    /// Assignment to an immutable binding in sloppy code: silently ignored (value stays on stack).
    GetName(u32),
    SetName(u32),
    /// Initialise / declare a binding by name at run time (eval / global code).
    TypeofName(u32),
    DeleteName(u32),
    /// Pushes [value, thisValue] for a call through a name (with-scope bases).
    GetNameThis(u32),
    /// Resolve a name now (for assignments whose RHS might change the scope): pushes a reference token.
    PushEnv(u32),
    PopEnv,
    /// Per-iteration copy of the current declarative environment.
    CopyEnv,
    /// obj -> (object environment for `with`)
    PushWith,
    GetImport(u32),
    GetArg(u32),
    /// Rest parameter: array of arguments from index n.
    RestArgs(u32),
    /// Arguments object: 0 unmapped, 1 mapped (operand: scope-slot list const or u32::MAX).
    Arguments(u32),
    This,
    /// `this` in a derived constructor before super(): ReferenceError if uninitialised.
    ThisChk,
    NewTarget,
    Callee,
    /// Bind the frame's `this` into a binding (after super() / at entry for captured this).
    LoadThisBinding,

    // ---- objects
    NewObject,
    NewArray(u32),
    /// arr value -> arr
    ArrayPush,
    ArrayHole,
    /// arr iterable -> arr
    ArraySpread,
    /// obj key value -> obj (CreateDataPropertyOrThrow)
    DefineField,
    /// obj value -> obj
    DefineFieldNamed(u32),
    /// obj key closure -> obj ; kind 0 method 1 getter 2 setter; +4 enumerable
    DefineMethod(u8),
    /// obj proto -> obj (object literal __proto__)
    SetProtoLit,
    /// target source -> target
    CopyDataProps,
    /// target source k1..kn -> target
    CopyDataPropsExcl(u32),
    GetProp(u32),
    /// obj value -> value
    SetProp(u32),
    GetElem,
    /// obj key value -> value
    SetElem,
    DeleteProp(u32),
    DeleteElem,
    /// key -> value (uses this + home object)
    GetSuper,
    /// key value -> value
    SetSuper,
    /// obj pn -> value
    GetPrivate,
    /// obj pn value -> value
    SetPrivate,
    /// pn obj -> bool
    PrivateIn,
    In,
    InstanceOf,
    ToPropertyKey,
    /// fn key -> base key: GetSuperBase of fn's [[HomeObject]], then ToPropertyKey(key).
    SuperBase,
    /// obj key -> obj key': RequireObjectCoercible(obj), then ToPropertyKey(key) (GetValue order).
    ToPropertyKeyChecked,
    ToNumeric,
    ToNumber,
    ToStringOp,
    ToObject,
    RequireObjectCoercible,

    // ---- operators
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    Neg,
    Pos,
    BitNot,
    Not,
    Inc,
    Dec,
    Typeof,
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    Lt,
    Le,
    Gt,
    Ge,

    // ---- control
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfTrue(u32),
    JumpIfFalseKeep(u32),
    JumpIfTrueKeep(u32),
    /// keep value; jump if not nullish
    JumpIfNotNullishKeep(u32),
    /// if top is nullish: pop it, push undefined, jump (optional chains); else keep
    JumpIfNullishUndef(u32),
    /// pop; jump if undefined
    JumpIfUndefined(u32),
    /// keep; jump if not undefined
    JumpIfNotUndefinedKeep(u32),

    // ---- calls
    Call(u32),
    CallSpread,
    New(u32),
    NewSpread,
    SuperCall(u32),
    SuperCallSpread,
    SuperCallForward,
    DirectEval(u32),
    DirectEvalSpread,
    Return,
    Throw,
    /// ReferenceError (operand: name const)
    ThrowRef(u32),
    /// TypeError (operand: message const)
    ThrowType(u32),
    /// SyntaxError at run time (operand: message const)
    ThrowSyntax(u32),
    PushHandler(u32),
    PopHandler,

    // ---- iteration
    /// obj -> iter next
    GetIterator,
    GetAsyncIterator,
    /// iter next -> iter next result (checks result is an object)
    IterNext,
    /// iter next -> iter next value | jump(done)
    IterStep(u32),
    /// iter next -> (calls return; checks object)
    IterClose,
    /// iter next -> (calls return; ignores errors)
    IterCloseQuiet,
    /// result -> bool / value
    IterResultDone,
    IterResultValue,
    /// obj -> enumerator
    ForInStart,
    /// enum -> enum key | jump(done; pops enum)
    ForInNext(u32),

    // ---- functions / classes
    Closure(u32),
    /// fn key -> fn ; 0 none 1 "get" 2 "set" prefix
    SetFunctionName(u8),
    /// closure home -> closure
    SetHome,
    /// heritage(or Empty) -> ctor proto ; operand: code const
    Class(u32),
    /// ctor key init -> ctor ; 0 instance field, 1 static field
    ClassField(u8),
    /// ctor pn closure -> ctor ; kind 0 method 1 getter 2 setter, +4 static
    ClassPrivateMethod(u8),
    /// ctor block -> ctor
    ClassStaticBlock,
    /// ctor -> ctor (runs static elements)
    ClassFinish,
    /// new PrivateName (operand: description const)
    NewPrivateName(u32),
    /// this-binding initialization of instance fields (frame's function's class data)
    InitFields,

    // ---- generators / async
    GenStart,
    Yield,
    /// after resume: dispatch next / throw / return(jump)
    GenDispatch(u32),
    Await,
    /// value -> {value, done}
    IterResult(bool),
    /// value -> awaited value with sync-iterator wrapping for async generators (yield in async gen)
    AsyncGenYield,

    // ---- misc
    Debugger,
    TemplateObject(u32),
    RegExp(u32),
    ImportCall,
    ImportMeta,
    /// Global declaration instantiation (operand: decls const)
    GlobalInit(u32),
    /// Eval declaration instantiation (operand: decls const)
    EvalInit(u32),
    /// Annex B.3.2: copy a block function into the var-scoped binding (operand: name const).
    BlockFnHoist(u32),
    /// Initialise a binding by name (script-level let/const/class, eval lexical): pops the value.
    InitName(u32),
    /// Resolve a name now (assignment through a dynamic scope): pushes a reference token.
    ResolveRef(u32),
    /// [token] -> [token value]
    GetRef(u32),
    /// [token value] -> [value]
    PutRef(u32),
    /// value thisRaw -> result (derived constructor return semantics)
    CheckDerivedReturn,
    /// iter next -> promise (async iterator close: calls return(), result to be awaited)
    AsyncIterClose,
    /// awaited return() result -> (TypeError if not an object)
    RequireObjectCoercibleResult,
    /// Global `this` of the current realm.
    GlobalThis,
    /// keep value; jump if nullish
    JumpIfNullishKeep(u32),
    /// value -> (initialise derived-constructor this binding; ReferenceError if already initialised)
    InitThisLocal(u32),
    InitThisEnv(u16, u32),
    /// yield* step 1: [received iter next] -> [kind result]; jumps (throw-without-method, return-without-method)
    YieldStarCall(u32, u32),
    /// yield* step 2: [kind result] -> value to yield | jump (done target, return target) with the value
    YieldStarCheck(u32, u32),
    /// Yield the top of stack without wrapping it in an iterator result (sync yield*).
    YieldRaw,
    Nop,

    // ---- explicit resource management (`using` / `await using`, ES2026): operand = the register holding the
    // scope's DisposeCapability
    /// register <- a new, empty DisposeCapability
    UsingNew(u32),
    /// value -> value: AddDisposableResource(cap, value, hint); true = async-dispose (`await using`)
    UsingAdd(u32, bool),
    /// kind value -> : a throw completion (kind 1) becomes the capability's pending error
    UsingSeed(u32),
    /// -> value to await | jump(done): disposes resources in reverse order until one needs an Await
    UsingStep(u32, u32),
    /// error -> : a rejected disposal Await joins the pending error (SuppressedError)
    UsingError(u32),
    /// -> : throw the pending error, if any
    UsingFinish(u32),
}

#[derive(Debug)]
pub enum BindKind {
    Var,
    Let,
    Const,
    Class,
    Func,
    Param,
    /// Function expression's own name: immutable, assignment ignored in sloppy, TypeError in strict.
    FnName,
    CatchParam,
    /// Internal bindings (this, new.target, home function, private names).
    Internal,
    Import,
}

/// Static description of a declarative environment record: binding names and kinds, by slot.
#[derive(Debug)]
pub struct ScopeInfo {
    pub names: Vec<JsStr>,
    pub kinds: Vec<BindKind>,
    /// A function's variable scope (direct sloppy eval adds var bindings here).
    pub var_scope: bool,
    /// Function scope of a function with mapped arguments etc.
    pub function: bool,
    /// Parameter scope of a function with parameter expressions (its body has a separate var scope): a direct
    /// eval in the parameters may not declare a var that shadows a parameter.
    pub params: bool,
}

impl ScopeInfo {
    pub fn find(&self, name: &JsStr) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }
}

#[derive(Debug)]
pub struct TemplateInfo {
    pub site: u32,
    pub cooked: Vec<Option<JsStr>>,
    pub raw: Vec<JsStr>,
}

/// GlobalDeclarationInstantiation / EvalDeclarationInstantiation input.
#[derive(Debug, Default)]
pub struct Decls {
    pub var_names: Vec<JsStr>,
    /// (name, closure code const index) for hoisted function declarations, in order.
    pub functions: Vec<(JsStr, u32)>,
    /// (name, is_const)
    pub lex: Vec<(JsStr, bool)>,
    /// Annex B.3.3 function names hoisted as vars (only created if allowed at run time).
    pub annexb_funcs: Vec<JsStr>,
    pub strict: bool,
}

#[derive(Debug)]
pub enum Const {
    Num(f64),
    Str(JsStr),
    BigInt(Rc<BigInt>),
    Code(Rc<Code>),
    Scope(Rc<ScopeInfo>),
    Template(Rc<TemplateInfo>),
    Regex(JsStr, JsStr),
    Decls(Rc<Decls>),
    Slots(Vec<u32>),
}

#[derive(Debug)]
pub struct SourceRef {
    pub src: Rc<[u16]>,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug)]
pub struct Code {
    pub name: JsStr,
    pub ops: Vec<Op>,
    pub consts: Vec<Const>,
    pub nlocals: u32,
    pub nparams: u32,
    pub length: u32,
    pub kind: FnKind,
    pub is_async: bool,
    pub is_generator: bool,
    pub strict: bool,
    pub simple_params: bool,
    pub derived: bool,
    /// A class constructor (base or derived) whose class has instance fields / private methods.
    pub has_fields: bool,
    pub source: Option<SourceRef>,
    /// JSPOS (SR127): the line table — bytecode offset to (line, column) of the source the op came from, for
    /// stack traces and error positions (delta-encoded, see [`LineTable`]).
    pub positions: LineTable,
    /// Scripts and modules: is this top-level module code?
    pub is_module: bool,
    pub is_script: bool,
    pub is_eval: bool,
    /// Module code: the static import / export description.
    pub module: Option<Rc<crate::vm::module::ModuleInfo>>,
}

impl Code {
    pub fn num(&self, i: u32) -> f64 {
        match &self.consts[i as usize] {
            Const::Num(n) => *n,
            _ => f64::NAN,
        }
    }
    pub fn str(&self, i: u32) -> &JsStr {
        match &self.consts[i as usize] {
            Const::Str(s) => s,
            _ => panic!("constant {} is not a string", i),
        }
    }
    pub fn is_constructor(&self) -> bool {
        matches!(self.kind, FnKind::Normal | FnKind::ClassConstructor) && !self.is_async && !self.is_generator
    }
}

// ------------------------------------------------------------------------------------------ JSPOS line table

/// Line starts of one source text (UTF-16 offsets), for offset -> (line, column). Line terminators are the
/// ECMAScript ones (§12.3): LF, CR (CR LF counts once), U+2028, U+2029.
#[derive(Clone, Debug)]
pub struct LineIndex {
    starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(src: &[u16]) -> LineIndex {
        let mut starts = alloc::vec![0u32];
        let mut i = 0usize;
        while i < src.len() {
            let c = src[i];
            if c == 0x0D && src.get(i + 1) == Some(&0x0A) {
                i += 1;
            }
            if matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029) {
                starts.push(i as u32 + 1);
            }
            i += 1;
        }
        LineIndex { starts }
    }

    /// 1-based (line, column) of a UTF-16 offset; the column counts UTF-16 code units, as V8's does.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let i = self.starts.partition_point(|&s| s <= offset).max(1) - 1;
        (i as u32 + 1, offset - self.starts[i] + 1)
    }
}

/// 1-based (line, column) of `offset` in `src` (a one-off: builds the index).
pub fn line_col(src: &[u16], offset: u32) -> (u32, u32) {
    LineIndex::new(src).line_col(offset)
}

/// JSPOS (SR127): one function's line table, built once by the compiler from the parser's token positions.
///
/// Encoding (`bytes`), one row per op whose position differs from the previous row's, rows in pc order:
/// `uvarint(pc - prev_pc)`, `zigzag-varint(line - prev_line)`, `uvarint(column)` (column absolute, 1-based;
/// `prev_pc` starts at 0, `prev_line` at 1). A row covers every op from its pc up to the next row's — the
/// SpiderMonkey/V8 source-note shape: one or two bytes for most rows, no per-op cost. `file` is the script's
/// URL (one `Rc` shared by every function of the script); `None` prints as `<anonymous>` (eval code,
/// `Function(...)` bodies, scripts run without a name).
#[derive(Clone, Debug, Default)]
pub struct LineTable {
    pub bytes: Vec<u8>,
    pub file: Option<Rc<str>>,
}

fn put_uvarint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_uvarint(b: &[u8], i: &mut usize) -> Option<u32> {
    let mut v = 0u32;
    let mut shift = 0;
    loop {
        let byte = *b.get(*i)?;
        *i += 1;
        v |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 28 {
            return None;
        }
    }
}

impl LineTable {
    /// An empty table naming `file` (a synthesized function: a default class constructor).
    pub fn empty(file: Option<Rc<str>>) -> LineTable {
        LineTable { bytes: Vec::new(), file }
    }

    /// Encode `rows` of (pc, source offset) — pc non-decreasing, as the compiler records them — through
    /// `index` into a table.
    pub fn build(rows: &[(u32, u32)], index: &LineIndex, file: Option<Rc<str>>) -> LineTable {
        let mut bytes = Vec::with_capacity(rows.len() * 3);
        let (mut ppc, mut pline, mut pcol) = (0u32, 1u32, 0u32);
        for &(pc, off) in rows {
            let (line, col) = index.line_col(off);
            if !bytes.is_empty() && line == pline && col == pcol {
                continue;
            }
            put_uvarint(&mut bytes, pc.saturating_sub(ppc));
            let d = line as i64 - pline as i64;
            put_uvarint(&mut bytes, ((d << 1) ^ (d >> 63)) as u32);
            put_uvarint(&mut bytes, col);
            (ppc, pline, pcol) = (pc, line, col);
        }
        bytes.shrink_to_fit();
        LineTable { bytes, file }
    }

    /// Every row as (pc, line, column), in pc order.
    pub fn rows(&self) -> Vec<(u32, u32, u32)> {
        let mut out = Vec::new();
        let (mut i, mut pc, mut line) = (0usize, 0u32, 1i64);
        while i < self.bytes.len() {
            let (Some(dpc), Some(dl), Some(col)) = (get_uvarint(&self.bytes, &mut i), get_uvarint(&self.bytes, &mut i), get_uvarint(&self.bytes, &mut i)) else { break };
            pc += dpc;
            line += ((dl >> 1) as i64) ^ -((dl & 1) as i64);
            out.push((pc, line as u32, col));
        }
        out
    }

    /// (line, column) of the op at `pc`: the last row at or before it.
    pub fn lookup(&self, pc: u32) -> Option<(u32, u32)> {
        let (mut i, mut rpc, mut line) = (0usize, 0u32, 1i64);
        let mut found = None;
        while i < self.bytes.len() {
            let (Some(dpc), Some(dl), Some(col)) = (get_uvarint(&self.bytes, &mut i), get_uvarint(&self.bytes, &mut i), get_uvarint(&self.bytes, &mut i)) else { break };
            rpc += dpc;
            if rpc > pc {
                break;
            }
            line += ((dl >> 1) as i64) ^ -((dl & 1) as i64);
            found = Some((line as u32, col));
        }
        found
    }

    /// The file name a stack frame prints: the script URL, or `<anonymous>`.
    pub fn file_name(&self) -> String {
        match &self.file {
            Some(f) => String::from(&**f),
            None => String::from("<anonymous>"),
        }
    }
}
