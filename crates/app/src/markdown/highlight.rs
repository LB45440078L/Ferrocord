//! Syntax highlighting for fenced code blocks (```lang).
//!
//! A small table-driven lexer instead of a full grammar engine: it recognises
//! comments, strings, numbers, keywords, types, function calls, annotations and
//! a few language-specific shapes (JSON keys, markup tags, diff lines, shell
//! variables). That is enough to make code readable in chat and keeps the
//! binary small. Unknown languages render as plain monospace text.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId};

use crate::theme::Palette;

/// Longer blocks are shown without colours to keep frames cheap.
const MAX_HIGHLIGHT: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Plain,
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Constant,
    Property,
    Annotation,
    Punct,
    Inserted,
    Deleted,
    Meta,
}

struct Colors {
    keyword: Color32,
    ty: Color32,
    function: Color32,
    string: Color32,
    number: Color32,
    comment: Color32,
    property: Color32,
    annotation: Color32,
    inserted: Color32,
    deleted: Color32,
    meta: Color32,
}

impl Colors {
    fn for_palette(pal: &Palette) -> Self {
        let bg = pal.bg_main;
        let dark = (bg.r() as u32 + bg.g() as u32 + bg.b() as u32) < 3 * 128;
        let h = crate::theme::hex;
        if dark {
            // One Dark-like.
            Self {
                keyword: h(0xc678dd),
                ty: h(0xe5c07b),
                function: h(0x61afef),
                string: h(0x98c379),
                number: h(0xd19a66),
                comment: h(0x7f848e),
                property: h(0xe06c75),
                annotation: h(0x56b6c2),
                inserted: h(0x98c379),
                deleted: h(0xe06c75),
                meta: h(0x56b6c2),
            }
        } else {
            // GitHub light-like.
            Self {
                keyword: h(0xcf222e),
                ty: h(0x953800),
                function: h(0x8250df),
                string: h(0x0a3069),
                number: h(0x0550ae),
                comment: h(0x6e7781),
                property: h(0x0550ae),
                annotation: h(0x116329),
                inserted: h(0x116329),
                deleted: h(0x82071e),
                meta: h(0x8250df),
            }
        }
    }

    fn get(&self, kind: Kind, pal: &Palette) -> Color32 {
        match kind {
            Kind::Plain => pal.text,
            Kind::Punct => pal.text_muted,
            Kind::Keyword => self.keyword,
            Kind::Type => self.ty,
            Kind::Function => self.function,
            Kind::String => self.string,
            Kind::Number | Kind::Constant => self.number,
            Kind::Comment => self.comment,
            Kind::Property => self.property,
            Kind::Annotation => self.annotation,
            Kind::Inserted => self.inserted,
            Kind::Deleted => self.deleted,
            Kind::Meta => self.meta,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// C-like and scripting languages, driven by a [`Spec`].
    Code,
    Json,
    Markup,
    Diff,
    /// key = value / key: value files (TOML, INI, YAML).
    Config,
}

struct Spec {
    mode: Mode,
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    /// Quote characters that start strings.
    quotes: &'static [char],
    /// Python-style `"""` / `'''` strings.
    triple_quotes: bool,
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    constants: &'static [&'static str],
    /// Identifiers starting with an upper-case letter are types (Java, Rust, ...).
    capitalized_types: bool,
    case_insensitive: bool,
    /// `@name` is an annotation/decorator.
    annotations: bool,
    /// `$name` / `${...}` are variables (shell, PHP, PowerShell).
    dollar_vars: bool,
    /// `name!` is a macro call and `'a` a lifetime (Rust).
    rust: bool,
}

const fn spec(mode: Mode) -> Spec {
    Spec {
        mode,
        line_comments: &[],
        block_comment: None,
        quotes: &['"', '\''],
        triple_quotes: false,
        keywords: &[],
        types: &[],
        constants: &[],
        capitalized_types: false,
        case_insensitive: false,
        annotations: false,
        dollar_vars: false,
        rust: false,
    }
}

const C_COMMENTS: (&[&str], Option<(&str, &str)>) = (&["//"], Some(("/*", "*/")));

const C_KEYWORDS: &[&str] = &[
    "auto",
    "break",
    "case",
    "const",
    "continue",
    "default",
    "do",
    "else",
    "enum",
    "extern",
    "for",
    "goto",
    "if",
    "inline",
    "register",
    "restrict",
    "return",
    "sizeof",
    "static",
    "struct",
    "switch",
    "typedef",
    "union",
    "volatile",
    "while",
    "class",
    "namespace",
    "template",
    "typename",
    "public",
    "private",
    "protected",
    "virtual",
    "override",
    "final",
    "new",
    "delete",
    "this",
    "throw",
    "try",
    "catch",
    "using",
    "operator",
    "friend",
    "constexpr",
    "noexcept",
    "nullptr",
    "static_cast",
    "dynamic_cast",
    "reinterpret_cast",
    "const_cast",
    "co_await",
    "co_return",
    "co_yield",
    "concept",
    "requires",
    "#include",
    "#define",
    "#ifdef",
    "#ifndef",
    "#endif",
    "#if",
    "#else",
    "#elif",
    "#pragma",
    "#undef",
];
const C_TYPES: &[&str] = &[
    "void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "bool", "size_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "int8_t",
    "int16_t", "int32_t", "int64_t", "wchar_t", "auto", "string", "std",
];
const C_CONSTANTS: &[&str] = &["true", "false", "NULL", "nullptr"];

const JAVA_KEYWORDS: &[&str] = &[
    "abstract",
    "assert",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "interface",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "var",
    "void",
    "volatile",
    "while",
    "record",
    "sealed",
    "permits",
    "non-sealed",
    "yield",
    // Kotlin / Scala / C# / Dart extras.
    "fun",
    "val",
    "when",
    "object",
    "companion",
    "data",
    "internal",
    "open",
    "override",
    "lateinit",
    "suspend",
    "is",
    "in",
    "out",
    "as",
    "def",
    "trait",
    "match",
    "using",
    "namespace",
    "readonly",
    "async",
    "await",
    "get",
    "set",
    "init",
    "struct",
    "ref",
    "params",
    "base",
    "foreach",
    "lock",
    "event",
    "delegate",
    "partial",
    "late",
    "required",
    "dynamic",
    "mixin",
    "extension",
    "with",
];
const JAVA_TYPES: &[&str] = &["boolean", "byte", "char", "short", "int", "long", "float", "double", "string", "decimal", "object", "uint", "ulong", "num"];
const JAVA_CONSTANTS: &[&str] = &["true", "false", "null", "Unit"];

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match",
    "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "type", "unsafe", "use", "where", "while", "yield",
];
const RUST_TYPES: &[&str] = &["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char", "str"];
const RUST_CONSTANTS: &[&str] = &["true", "false", "None", "Some", "Ok", "Err"];

const GO_KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];
const GO_TYPES: &[&str] = &[
    "bool",
    "byte",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
    "any",
];
const GO_CONSTANTS: &[&str] = &["true", "false", "nil", "iota"];

const JS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "as",
    "interface",
    "type",
    "enum",
    "implements",
    "private",
    "public",
    "protected",
    "readonly",
    "declare",
    "namespace",
    "abstract",
    "keyof",
    "satisfies",
];
const JS_TYPES: &[&str] = &["string", "number", "boolean", "any", "unknown", "never", "object", "symbol", "bigint", "void"];
const JS_CONSTANTS: &[&str] = &["true", "false", "null", "undefined", "NaN", "Infinity"];

const PY_KEYWORDS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if",
    "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "match", "case", "self", "cls",
];
const PY_TYPES: &[&str] = &["int", "float", "str", "bool", "list", "dict", "set", "tuple", "bytes", "object", "type", "complex", "frozenset"];
const PY_CONSTANTS: &[&str] = &["True", "False", "None"];

const RUBY_KEYWORDS: &[&str] = &[
    "alias",
    "and",
    "begin",
    "break",
    "case",
    "class",
    "def",
    "defined?",
    "do",
    "else",
    "elsif",
    "end",
    "ensure",
    "for",
    "if",
    "in",
    "module",
    "next",
    "not",
    "or",
    "redo",
    "rescue",
    "retry",
    "return",
    "self",
    "super",
    "then",
    "undef",
    "unless",
    "until",
    "when",
    "while",
    "yield",
    "require",
    "attr_accessor",
    "puts",
];
const RUBY_CONSTANTS: &[&str] = &["true", "false", "nil"];

const PHP_KEYWORDS: &[&str] = &[
    "abstract",
    "and",
    "as",
    "break",
    "case",
    "catch",
    "class",
    "clone",
    "const",
    "continue",
    "declare",
    "default",
    "do",
    "echo",
    "else",
    "elseif",
    "empty",
    "enddeclare",
    "endfor",
    "endforeach",
    "endif",
    "endswitch",
    "endwhile",
    "extends",
    "final",
    "finally",
    "fn",
    "for",
    "foreach",
    "function",
    "global",
    "if",
    "implements",
    "include",
    "instanceof",
    "interface",
    "isset",
    "list",
    "match",
    "namespace",
    "new",
    "or",
    "print",
    "private",
    "protected",
    "public",
    "require",
    "return",
    "static",
    "switch",
    "throw",
    "trait",
    "try",
    "unset",
    "use",
    "var",
    "while",
    "yield",
];
const PHP_CONSTANTS: &[&str] = &["true", "false", "null", "TRUE", "FALSE", "NULL"];

const SWIFT_KEYWORDS: &[&str] = &[
    "associatedtype",
    "class",
    "deinit",
    "enum",
    "extension",
    "fileprivate",
    "func",
    "import",
    "init",
    "inout",
    "internal",
    "let",
    "open",
    "operator",
    "private",
    "protocol",
    "public",
    "static",
    "struct",
    "subscript",
    "typealias",
    "var",
    "break",
    "case",
    "continue",
    "default",
    "defer",
    "do",
    "else",
    "fallthrough",
    "for",
    "guard",
    "if",
    "in",
    "repeat",
    "return",
    "switch",
    "where",
    "while",
    "as",
    "catch",
    "is",
    "rethrows",
    "throw",
    "throws",
    "try",
    "async",
    "await",
    "self",
    "Self",
    "some",
    "any",
];
const SWIFT_CONSTANTS: &[&str] = &["true", "false", "nil"];

const LUA_KEYWORDS: &[&str] =
    &["and", "break", "do", "else", "elseif", "end", "for", "function", "goto", "if", "in", "local", "not", "or", "repeat", "return", "then", "until", "while"];
const LUA_CONSTANTS: &[&str] = &["true", "false", "nil"];

const SQL_KEYWORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "and",
    "or",
    "not",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "table",
    "drop",
    "alter",
    "add",
    "index",
    "on",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "full",
    "cross",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "offset",
    "as",
    "distinct",
    "union",
    "all",
    "case",
    "when",
    "then",
    "else",
    "end",
    "in",
    "is",
    "like",
    "between",
    "exists",
    "primary",
    "key",
    "foreign",
    "references",
    "default",
    "unique",
    "view",
    "with",
    "returning",
    "asc",
    "desc",
    "begin",
    "commit",
    "rollback",
    "transaction",
    "if",
    "constraint",
    "check",
];
const SQL_TYPES: &[&str] = &[
    "int",
    "integer",
    "bigint",
    "smallint",
    "text",
    "varchar",
    "char",
    "boolean",
    "date",
    "timestamp",
    "float",
    "real",
    "numeric",
    "decimal",
    "blob",
    "serial",
];
const SQL_CONSTANTS: &[&str] = &["null", "true", "false"];

const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "return", "exit", "export", "local",
    "readonly", "set", "unset", "source", "alias", "echo", "cd", "sudo", "shift", "trap", "eval", "exec", // PowerShell
    "param", "foreach", "switch", "try", "catch", "finally", "throw", "begin", "process", "end",
];
const SHELL_CONSTANTS: &[&str] = &["true", "false", "$true", "$false", "$null"];

const CSS_KEYWORDS: &[&str] = &["@media", "@import", "@keyframes", "@font-face", "@supports", "!important", "from", "to"];

const CONFIG_CONSTANTS: &[&str] = &["true", "false", "null", "yes", "no", "on", "off", "~"];

fn lookup(lang: &str) -> Option<Spec> {
    let lang = lang.trim().to_ascii_lowercase();
    let (lc, bc) = C_COMMENTS;
    let s = match lang.as_str() {
        "c" | "h" | "cpp" | "c++" | "cc" | "cxx" | "hpp" | "hh" | "objc" | "arduino" | "ino" => {
            Spec { line_comments: lc, block_comment: bc, keywords: C_KEYWORDS, types: C_TYPES, constants: C_CONSTANTS, ..spec(Mode::Code) }
        }
        "java" | "kotlin" | "kt" | "kts" | "scala" | "cs" | "csharp" | "c#" | "dart" | "groovy" | "gradle" => Spec {
            line_comments: lc,
            block_comment: bc,
            keywords: JAVA_KEYWORDS,
            types: JAVA_TYPES,
            constants: JAVA_CONSTANTS,
            capitalized_types: true,
            annotations: true,
            ..spec(Mode::Code)
        },
        "rust" | "rs" => Spec {
            line_comments: lc,
            block_comment: bc,
            quotes: &['"'],
            keywords: RUST_KEYWORDS,
            types: RUST_TYPES,
            constants: RUST_CONSTANTS,
            capitalized_types: true,
            rust: true,
            ..spec(Mode::Code)
        },
        "go" | "golang" => Spec {
            line_comments: lc,
            block_comment: bc,
            quotes: &['"', '\'', '`'],
            keywords: GO_KEYWORDS,
            types: GO_TYPES,
            constants: GO_CONSTANTS,
            capitalized_types: false,
            ..spec(Mode::Code)
        },
        "js" | "javascript" | "jsx" | "mjs" | "cjs" | "ts" | "typescript" | "tsx" | "node" => Spec {
            line_comments: lc,
            block_comment: bc,
            quotes: &['"', '\'', '`'],
            keywords: JS_KEYWORDS,
            types: JS_TYPES,
            constants: JS_CONSTANTS,
            capitalized_types: true,
            annotations: true,
            ..spec(Mode::Code)
        },
        "py" | "python" | "python3" | "py3" | "gdscript" => Spec {
            line_comments: &["#"],
            triple_quotes: true,
            keywords: PY_KEYWORDS,
            types: PY_TYPES,
            constants: PY_CONSTANTS,
            capitalized_types: true,
            annotations: true,
            ..spec(Mode::Code)
        },
        "rb" | "ruby" => Spec { line_comments: &["#"], keywords: RUBY_KEYWORDS, constants: RUBY_CONSTANTS, capitalized_types: true, ..spec(Mode::Code) },
        "php" => Spec {
            line_comments: &["//", "#"],
            block_comment: bc,
            keywords: PHP_KEYWORDS,
            constants: PHP_CONSTANTS,
            capitalized_types: true,
            dollar_vars: true,
            ..spec(Mode::Code)
        },
        "swift" => Spec {
            line_comments: lc,
            block_comment: bc,
            quotes: &['"'],
            keywords: SWIFT_KEYWORDS,
            constants: SWIFT_CONSTANTS,
            capitalized_types: true,
            annotations: true,
            ..spec(Mode::Code)
        },
        "lua" | "luau" => {
            Spec { line_comments: &["--"], block_comment: Some(("--[[", "]]")), keywords: LUA_KEYWORDS, constants: LUA_CONSTANTS, ..spec(Mode::Code) }
        }
        "sql" | "mysql" | "postgres" | "postgresql" | "sqlite" | "plsql" | "tsql" => Spec {
            line_comments: &["--"],
            block_comment: bc,
            keywords: SQL_KEYWORDS,
            types: SQL_TYPES,
            constants: SQL_CONSTANTS,
            case_insensitive: true,
            ..spec(Mode::Code)
        },
        "sh" | "bash" | "shell" | "zsh" | "fish" | "console" | "shellsession" | "ps1" | "powershell" | "pwsh" | "bat" | "cmd" => Spec {
            line_comments: &["#"],
            keywords: SHELL_KEYWORDS,
            constants: SHELL_CONSTANTS,
            dollar_vars: true,
            case_insensitive: matches!(lang.as_str(), "ps1" | "powershell" | "pwsh" | "bat" | "cmd"),
            ..spec(Mode::Code)
        },
        "css" | "scss" | "sass" | "less" => {
            Spec { line_comments: &["//"], block_comment: bc, keywords: CSS_KEYWORDS, annotations: false, ..spec(Mode::Config) }
        }
        "json" | "jsonc" | "json5" | "geojson" => Spec { line_comments: lc, block_comment: bc, quotes: &['"'], ..spec(Mode::Json) },
        "html" | "htm" | "xml" | "svg" | "xhtml" | "vue" | "svelte" | "plist" | "xaml" | "csproj" => spec(Mode::Markup),
        "diff" | "patch" => spec(Mode::Diff),
        "toml" | "ini" | "cfg" | "conf" | "yaml" | "yml" | "properties" | "env" | "dotenv" | "editorconfig" | "gitconfig" => {
            Spec { line_comments: &["#", ";"], constants: CONFIG_CONSTANTS, ..spec(Mode::Config) }
        }
        _ => return None,
    };
    Some(s)
}

/// Whether `lang` gets colours.
#[cfg(test)]
pub fn is_supported(lang: &str) -> bool {
    lookup(lang).is_some()
}

/// Splits `code` into coloured tokens. Exposed for tests.
pub fn tokens(code: &str, lang: &str) -> Vec<(std::ops::Range<usize>, Kind)> {
    let Some(spec) = lookup(lang).filter(|_| code.len() <= MAX_HIGHLIGHT) else { return vec![(0..code.len(), Kind::Plain)] };
    let mut lx = Lexer { src: code, pos: 0, out: Vec::new() };
    match spec.mode {
        Mode::Code => lx.code(&spec),
        Mode::Json => lx.json(&spec),
        Mode::Markup => lx.markup(),
        Mode::Diff => lx.diff(),
        Mode::Config => lx.config(&spec),
    }
    lx.out
}

/// Builds a coloured, monospace layout job for a code block.
pub fn highlight(code: &str, lang: &str, size: f32, pal: &Palette) -> LayoutJob {
    let colors = Colors::for_palette(pal);
    let font = FontId::new(size, FontFamily::Monospace);
    let mut job = LayoutJob::default();
    for (range, kind) in tokens(code, lang) {
        job.append(&code[range], 0.0, TextFormat { font_id: font.clone(), color: colors.get(kind, pal), ..Default::default() });
    }
    job
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    out: Vec<(std::ops::Range<usize>, Kind)>,
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl<'a> Lexer<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn emit(&mut self, end: usize, kind: Kind) {
        if end <= self.pos {
            return;
        }
        match self.out.last_mut() {
            Some((r, k)) if *k == kind && r.end == self.pos => r.end = end,
            _ => self.out.push((self.pos..end, kind)),
        }
        self.pos = end;
    }

    /// Emits up to (not including) the next newline.
    fn rest_of_line(&mut self, kind: Kind) {
        let end = self.rest().find('\n').map(|i| self.pos + i).unwrap_or(self.src.len());
        self.emit(end, kind);
    }

    fn string(&mut self, quote: &str, multiline: bool) {
        let body = self.pos + quote.len();
        let mut i = body;
        let bytes = self.src.as_bytes();
        while i < self.src.len() {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if self.src[i..].starts_with(quote) {
                i += quote.len();
                self.emit(i.min(self.src.len()), Kind::String);
                return;
            }
            if bytes[i] == b'\n' && !multiline {
                break;
            }
            i += 1;
        }
        // Unterminated: colour to the end of the line (or input).
        let end = i.min(self.src.len());
        self.emit(end, Kind::String);
    }

    fn number(&mut self) {
        let mut end = self.pos;
        let mut prev = ' ';
        for (i, c) in self.rest().char_indices() {
            let sign = (c == '+' || c == '-') && (i == 0 || (matches!(prev, 'e' | 'E') && !self.rest().starts_with("0x")));
            let ok = c.is_ascii_alphanumeric() || c == '_' || (c == '.' && prev != '.') || sign;
            if !ok {
                break;
            }
            // `1..10` is a range, not a number with two dots.
            if c == '.' && self.rest()[i + 1..].starts_with('.') {
                break;
            }
            prev = c;
            end = self.pos + i + c.len_utf8();
        }
        // Always consume at least one character so callers can't loop forever.
        let end = end.max(self.pos + self.peek().map_or(0, char::len_utf8));
        self.emit(end, Kind::Number);
    }

    fn word_end(&self) -> usize {
        self.rest().char_indices().find(|(_, c)| !is_ident_char(*c)).map(|(i, _)| self.pos + i).unwrap_or(self.src.len())
    }

    fn code(&mut self, s: &Spec) {
        let has = |list: &[&str], w: &str| if s.case_insensitive { list.iter().any(|k| k.eq_ignore_ascii_case(w)) } else { list.contains(&w) };
        while let Some(c) = self.peek() {
            let rest = self.rest();
            if let Some((open, close)) = s.block_comment
                && rest.starts_with(open)
            {
                let end = rest[open.len()..].find(close).map(|i| self.pos + open.len() + i + close.len()).unwrap_or(self.src.len());
                self.emit(end, Kind::Comment);
                continue;
            }
            if s.line_comments.iter().any(|lc| rest.starts_with(lc)) && !(s.dollar_vars && rest.starts_with("#!") && self.pos == 0) {
                self.rest_of_line(Kind::Comment);
                continue;
            }
            if s.triple_quotes && (rest.starts_with("\"\"\"") || rest.starts_with("'''")) {
                let q = &rest[..3];
                self.string(q, true);
                continue;
            }
            if s.rust && c == '\'' {
                // Char literal ('a', '\n') or lifetime ('a).
                let after: Vec<char> = rest.chars().take(4).collect();
                let is_char = matches!(after.as_slice(), ['\'', '\\', ..]) || matches!(after.as_slice(), ['\'', _, '\'', ..]);
                if is_char {
                    self.string("'", false);
                } else {
                    let end = self.pos + 1 + rest[1..].char_indices().find(|(_, c)| !is_ident_char(*c)).map(|(i, _)| i).unwrap_or(rest.len() - 1);
                    self.emit(end, Kind::Keyword);
                }
                continue;
            }
            if s.quotes.contains(&c) {
                let q = &rest[..c.len_utf8()];
                self.string(q, c == '`');
                continue;
            }
            if c.is_ascii_digit() || (c == '.' && rest[1..].starts_with(|d: char| d.is_ascii_digit())) {
                self.number();
                continue;
            }
            if s.annotations && c == '@' && rest[1..].starts_with(is_ident_start) {
                self.pos += 1;
                let end = self.word_end();
                self.pos -= 1;
                self.emit(end, Kind::Annotation);
                continue;
            }
            if s.dollar_vars && c == '$' {
                let end = if rest[1..].starts_with('{') {
                    rest.find('}').map(|i| self.pos + i + 1).unwrap_or(self.pos + 1)
                } else {
                    self.pos + 1 + rest[1..].char_indices().find(|(_, c)| !is_ident_char(*c)).map(|(i, _)| i).unwrap_or(rest.len() - 1)
                };
                let word = &self.src[self.pos..end];
                let kind = if has(s.constants, word) { Kind::Constant } else { Kind::Property };
                self.emit(end.max(self.pos + 1), kind);
                continue;
            }
            if c == '#' && !s.line_comments.contains(&"#") {
                // C preprocessor directives.
                let end = self.pos + 1 + rest[1..].char_indices().find(|(_, c)| !is_ident_char(*c)).map(|(i, _)| i).unwrap_or(rest.len() - 1);
                if has(s.keywords, &self.src[self.pos..end]) {
                    self.emit(end, Kind::Meta);
                    continue;
                }
            }
            if is_ident_start(c) {
                let end = self.word_end();
                let word = &self.src[self.pos..end];
                let next = self.src[end..].trim_start_matches([' ', '\t']).chars().next();
                let kind = if has(s.keywords, word) {
                    Kind::Keyword
                } else if has(s.constants, word) {
                    Kind::Constant
                } else if has(s.types, word) {
                    Kind::Type
                } else if s.rust && self.src[end..].starts_with('!') && !self.src[end..].starts_with("!=") {
                    Kind::Annotation
                } else if next == Some('(') {
                    Kind::Function
                } else if s.capitalized_types && word.starts_with(|c: char| c.is_uppercase()) {
                    // ALL_CAPS reads as a constant, CamelCase as a type.
                    if word.len() > 1 && word.chars().all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_') { Kind::Constant } else { Kind::Type }
                } else {
                    Kind::Plain
                };
                self.emit(end, kind);
                continue;
            }
            let end = self.pos + c.len_utf8();
            let kind = if c.is_whitespace() || c.is_alphanumeric() { Kind::Plain } else { Kind::Punct };
            self.emit(end, kind);
        }
    }

    fn json(&mut self, s: &Spec) {
        while let Some(c) = self.peek() {
            let rest = self.rest();
            if let Some((open, close)) = s.block_comment
                && rest.starts_with(open)
            {
                let end = rest[open.len()..].find(close).map(|i| self.pos + open.len() + i + close.len()).unwrap_or(self.src.len());
                self.emit(end, Kind::Comment);
                continue;
            }
            if rest.starts_with("//") {
                self.rest_of_line(Kind::Comment);
                continue;
            }
            if c == '"' {
                let start = self.pos;
                self.string("\"", false);
                // A string followed by ':' is a key.
                if self.rest().trim_start().starts_with(':')
                    && let Some((r, k)) = self.out.last_mut()
                    && r.start == start
                {
                    *k = Kind::Property;
                }
                continue;
            }
            if c == '-' || c.is_ascii_digit() {
                self.number();
                continue;
            }
            if is_ident_start(c) {
                let end = self.word_end();
                let kind = if matches!(&self.src[self.pos..end], "true" | "false" | "null") { Kind::Constant } else { Kind::Plain };
                self.emit(end, kind);
                continue;
            }
            let kind = if c.is_whitespace() { Kind::Plain } else { Kind::Punct };
            self.emit(self.pos + c.len_utf8(), kind);
        }
    }

    fn markup(&mut self) {
        while let Some(c) = self.peek() {
            let rest = self.rest();
            if rest.starts_with("<!--") {
                let end = rest.find("-->").map(|i| self.pos + i + 3).unwrap_or(self.src.len());
                self.emit(end, Kind::Comment);
                continue;
            }
            if c == '<' {
                // Tag: punctuation, name, attributes, strings.
                let close = rest.find('>').map(|i| self.pos + i + 1).unwrap_or(self.src.len());
                let mut i = self.pos + 1;
                if self.src[i..].starts_with(['/', '!', '?']) {
                    i += 1;
                }
                self.emit(i, Kind::Punct);
                let name_end = self.src[i..close].find(|c: char| c.is_whitespace() || c == '>' || c == '/').map(|j| i + j).unwrap_or(close);
                self.emit(name_end, Kind::Keyword);
                while self.pos < close {
                    let c = self.peek().unwrap_or('>');
                    if c == '"' || c == '\'' {
                        let q = &self.src[self.pos..self.pos + 1];
                        self.string(q, true);
                    } else if is_ident_start(c) {
                        let end = self.src[self.pos..close].find(|c: char| !(is_ident_char(c) || c == '-' || c == ':')).map(|j| self.pos + j).unwrap_or(close);
                        self.emit(end, Kind::Property);
                    } else {
                        let kind = if c.is_whitespace() { Kind::Plain } else { Kind::Punct };
                        self.emit(self.pos + c.len_utf8(), kind);
                    }
                }
                continue;
            }
            if c == '&' {
                let end = rest.find(';').filter(|i| *i < 10).map(|i| self.pos + i + 1).unwrap_or(self.pos + 1);
                self.emit(end, Kind::Constant);
                continue;
            }
            let end = rest.find(['<', '&']).map(|i| self.pos + i).unwrap_or(self.src.len()).max(self.pos + c.len_utf8());
            self.emit(end, Kind::Plain);
        }
    }

    fn diff(&mut self) {
        while self.pos < self.src.len() {
            let rest = self.rest();
            let kind = if rest.starts_with("+++") || rest.starts_with("---") || rest.starts_with("diff ") || rest.starts_with("index ") {
                Kind::Meta
            } else if rest.starts_with("@@") {
                Kind::Annotation
            } else if rest.starts_with('+') || rest.starts_with('>') {
                Kind::Inserted
            } else if rest.starts_with('-') || rest.starts_with('<') {
                Kind::Deleted
            } else {
                Kind::Plain
            };
            self.rest_of_line(kind);
            if self.pos < self.src.len() {
                self.emit(self.pos + 1, Kind::Plain);
            }
        }
    }

    fn config(&mut self, s: &Spec) {
        let mut line_start = true;
        while let Some(c) = self.peek() {
            let rest = self.rest();
            if c == '\n' {
                self.emit(self.pos + 1, Kind::Plain);
                line_start = true;
                continue;
            }
            if line_start && (c == ' ' || c == '\t') {
                self.emit(self.pos + 1, Kind::Plain);
                continue;
            }
            if let Some((open, close)) = s.block_comment
                && rest.starts_with(open)
            {
                let end = rest[open.len()..].find(close).map(|i| self.pos + open.len() + i + close.len()).unwrap_or(self.src.len());
                self.emit(end, Kind::Comment);
                continue;
            }
            if s.line_comments.iter().any(|lc| rest.starts_with(lc)) && (line_start || self.src[..self.pos].ends_with([' ', '\t'])) {
                self.rest_of_line(Kind::Comment);
                continue;
            }
            if line_start && c == '[' {
                // [section] / [[table]]
                let end = rest.find('\n').map(|i| self.pos + i).unwrap_or(self.src.len());
                self.emit(end, Kind::Type);
                continue;
            }
            if line_start {
                line_start = false;
                // key = / key: (also CSS `property:` and YAML `- key:`)
                let line_end = rest.find('\n').unwrap_or(rest.len());
                let line = &rest[..line_end];
                let lead = if line.starts_with("- ") { 2 } else { 0 };
                if let Some(sep) = line.find([':', '=']).filter(|i| *i > lead && !line[lead..*i].trim().is_empty() && !line[lead..*i].contains(['"', '{', '(']))
                {
                    let base = self.pos;
                    self.emit(base + lead, Kind::Punct);
                    self.emit(base + lead + line[lead..sep].trim_end().len(), Kind::Property);
                    continue;
                }
                continue;
            }
            if c == '"' || c == '\'' {
                let q = &rest[..1];
                self.string(q, false);
                continue;
            }
            if c.is_ascii_digit() || (c == '-' && rest[1..].starts_with(|d: char| d.is_ascii_digit())) {
                self.number();
                continue;
            }
            if c == '#' && rest[1..].starts_with(|d: char| d.is_ascii_hexdigit()) {
                // CSS colours.
                let end = self.pos + 1 + rest[1..].char_indices().find(|(_, c)| !c.is_ascii_hexdigit()).map(|(i, _)| i).unwrap_or(rest.len() - 1);
                self.emit(end, Kind::Number);
                continue;
            }
            if is_ident_start(c) || c == '@' || c == '!' {
                let start = self.pos;
                self.pos += c.len_utf8();
                let end = self.rest().char_indices().find(|(_, c)| !(is_ident_char(*c) || *c == '-')).map(|(i, _)| self.pos + i).unwrap_or(self.src.len());
                self.pos = start;
                let word = &self.src[start..end];
                let kind = if s.constants.iter().any(|k| k.eq_ignore_ascii_case(word)) {
                    Kind::Constant
                } else if s.keywords.contains(&word) {
                    Kind::Keyword
                } else if self.src[end..].starts_with('(') {
                    Kind::Function
                } else {
                    Kind::Plain
                };
                self.emit(end, kind);
                continue;
            }
            let kind = if c.is_whitespace() { Kind::Plain } else { Kind::Punct };
            self.emit(self.pos + c.len_utf8(), kind);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(code: &str, lang: &str) -> Vec<(String, Kind)> {
        tokens(code, lang).into_iter().filter(|(_, k)| *k != Kind::Plain && *k != Kind::Punct).map(|(r, k)| (code[r].to_string(), k)).collect()
    }

    fn covers(code: &str, lang: &str) {
        let t = tokens(code, lang);
        let mut at = 0;
        for (r, _) in &t {
            assert_eq!(r.start, at, "gap or overlap in {lang}: {t:?}");
            at = r.end;
        }
        assert_eq!(at, code.len(), "tokens do not cover {lang} input");
    }

    #[test]
    fn java() {
        let k = kinds("@Override\npublic static int add(int a) { return a + 1; } // hi", "java");
        assert!(k.contains(&("@Override".into(), Kind::Annotation)));
        assert!(k.contains(&("public".into(), Kind::Keyword)));
        assert!(k.contains(&("int".into(), Kind::Type)));
        assert!(k.contains(&("add".into(), Kind::Function)));
        assert!(k.contains(&("1".into(), Kind::Number)));
        assert!(k.contains(&("// hi".into(), Kind::Comment)));
    }

    #[test]
    fn python() {
        let k = kinds("def f(x):\n    \"\"\"doc\nstring\"\"\"\n    return None  # done", "py");
        assert!(k.contains(&("def".into(), Kind::Keyword)));
        assert!(k.contains(&("f".into(), Kind::Function)));
        assert!(k.contains(&("\"\"\"doc\nstring\"\"\"".into(), Kind::String)));
        assert!(k.contains(&("None".into(), Kind::Constant)));
        assert!(k.contains(&("# done".into(), Kind::Comment)));
    }

    #[test]
    fn rust_lifetimes_chars_and_macros() {
        let k = kinds("fn f<'a>(s: &'a str) -> char { println!(\"{}\", s); 'x' }", "rs");
        assert!(k.contains(&("'a".into(), Kind::Keyword)));
        assert!(k.contains(&("'x'".into(), Kind::String)));
        assert!(k.contains(&("println".into(), Kind::Annotation)));
        assert!(k.contains(&("str".into(), Kind::Type)));
    }

    #[test]
    fn json_keys_and_values() {
        let k = kinds("{\"name\": \"ferrocord\", \"ok\": true, \"n\": -1.5e3}", "json");
        assert!(k.contains(&("\"name\"".into(), Kind::Property)));
        assert!(k.contains(&("\"ferrocord\"".into(), Kind::String)));
        assert!(k.contains(&("true".into(), Kind::Constant)));
        assert!(k.contains(&("-1.5e3".into(), Kind::Number)));
    }

    #[test]
    fn diff_lines() {
        let k = kinds("@@ -1 +1 @@\n-old\n+new\n same", "diff");
        assert_eq!(k, vec![("@@ -1 +1 @@".into(), Kind::Annotation), ("-old".into(), Kind::Deleted), ("+new".into(), Kind::Inserted)]);
    }

    #[test]
    fn markup_and_config() {
        let k = kinds("<a href=\"x\">&amp;</a>", "html");
        assert!(k.contains(&("a".into(), Kind::Keyword)));
        assert!(k.contains(&("href".into(), Kind::Property)));
        assert!(k.contains(&("\"x\"".into(), Kind::String)));
        let k = kinds("[package]\nname = \"x\" # c\nok = true", "toml");
        assert!(k.contains(&("[package]".into(), Kind::Type)));
        assert!(k.contains(&("name".into(), Kind::Property)));
        assert!(k.contains(&("true".into(), Kind::Constant)));
        assert!(k.contains(&("# c".into(), Kind::Comment)));
    }

    #[test]
    fn tokens_cover_input_without_gaps() {
        let samples = [
            ("java", "class A { String s = \"unterminated\n int x = 0x1F; }"),
            ("py", "x = '''open"),
            ("rs", "let r = 1..10; let s = \"\\\"\"; // é ünïcödé"),
            ("js", "const t = `a ${b} c`; /* open"),
            ("sql", "SELECT * FROM t WHERE a = 'x' -- c"),
            ("bash", "#!/bin/bash\necho \"$HOME\" ${PATH} # c"),
            ("yaml", "- key: value\n  other: 12 # c"),
            ("css", ".a { color: #fff; margin: 0 auto !important; }"),
            ("html", "<div class='x'>text &lt; <!-- c --></div><open"),
            ("json", "{\"a\": [1, 2, {\"b\": null}]"),
            ("diff", "+a\n-b"),
            ("lua", "--[[ block ]] local x = nil -- c"),
            ("unknown", "plain"),
        ];
        for (lang, code) in samples {
            covers(code, lang);
        }
    }

    #[test]
    fn unknown_language_is_plain() {
        assert!(!is_supported("brainfuck"));
        assert_eq!(tokens("x", "brainfuck"), vec![(0..1, Kind::Plain)]);
    }
}
