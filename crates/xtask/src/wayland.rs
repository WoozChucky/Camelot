//! The Wayland protocol tables (#184): the official protocol descriptions,
//! `protocols/wayland.xml` (Wayland 1.23.0) and `protocols/xdg-shell.xml`
//! (wayland-protocols 1.38), and wlroots' layer shell and foreign toplevel
//! management (`protocols/wlr-*.xml`, the desktop shell's: #185), turned into C for the GUI library and the
//! display server, as libwayland's `wayland-scanner` turns them into
//! `wayland-protocol.c` and the protocol headers.
//!
//! - `kernel/user/gui/protocol.h`: libwayland's `struct wl_message` and
//!   `struct wl_interface`, every interface's `extern`, and for each its
//!   requests' and events' opcodes (`WL_SURFACE_ATTACH`, `XDG_SURFACE_CONFIGURE`:
//!   wayland-scanner's names), the version each came in
//!   (`WL_SURFACE_DAMAGE_BUFFER_SINCE_VERSION`) and its enums' values
//!   (`WL_SHM_FORMAT_XRGB8888`).
//! - `kernel/user/gui/protocol.c`: the tables. A message's signature is
//!   libwayland's: the version it came in when that is past 1, then for each
//!   argument `?` when it may be null and a letter, `i` int, `u` uint, `f`
//!   fixed, `s` string, `o` object, `n` new_id, `a` array, `h` a file
//!   descriptor; a new_id with no interface (`wl_registry.bind`'s) is
//!   `sun`, the interface's name and version first. Its types are the
//!   interface of each object or new_id argument, NULL for the others.
//!
//! `cargo xtask kernel` writes them (they are committed); a test checks
//! they are current, and the XML files' checksums, so that the tables
//! follow the files and the files their releases. The XML is read by a
//! small parser of the part of XML these files use: elements, attributes,
//! comments, text and the five predefined entities.

use std::path::Path;

/// The protocol files, under the repository root, in the order generated.
pub const XML: &[&str] = &[
    "protocols/wayland.xml",
    "protocols/xdg-shell.xml",
    "protocols/wlr-layer-shell-unstable-v1.xml",
    "protocols/wlr-foreign-toplevel-management-unstable-v1.xml",
];
/// The generated files, under the repository root.
pub const HEADER: &str = "kernel/user/gui/protocol.h";
pub const SOURCE: &str = "kernel/user/gui/protocol.c";

// ---- XML ----

/// An element: its name, its attributes in order, its child elements (text
/// and comments are dropped: the descriptions).
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
}

impl Element {
    /// The attribute `name`'s value.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The children named `name`.
    pub fn each<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    /// Where pos is, as LINE:COL, for errors.
    fn at(&self) -> String {
        let before = &self.s[..self.pos];
        let line = before.matches('\n').count() + 1;
        let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
        format!("{line}:{col}")
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{}: {what}", self.at()))
    }

    fn skip_space(&mut self) {
        let n = self.rest().len() - self.rest().trim_start().len();
        self.pos += n;
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.rest().starts_with(lit) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    /// Skips past `end`, which must come.
    fn skip_past(&mut self, end: &str) -> Result<(), String> {
        match self.rest().find(end) {
            Some(i) => {
                self.pos += i + end.len();
                Ok(())
            }
            None => self.err(&format!("no {end}")),
        }
    }

    fn name(&mut self) -> Result<String, String> {
        let n = self
            .rest()
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.')))
            .unwrap_or(self.rest().len());
        if n == 0 {
            return self.err("a name expected");
        }
        let name = self.rest()[..n].to_string();
        self.pos += n;
        Ok(name)
    }

    /// Text up to `end`, its entities replaced.
    fn text_until(&mut self, end: char) -> Result<String, String> {
        let Some(n) = self.rest().find(end) else {
            return self.err(&format!("no {end}"));
        };
        let raw = &self.rest()[..n];
        self.pos += n;
        unescape(raw).map_err(|e| format!("{}: {e}", self.at()))
    }

    /// Comments, processing instructions and text, up to the next tag.
    fn skip_misc(&mut self) -> Result<(), String> {
        loop {
            match self.rest().find('<') {
                Some(i) => self.pos += i,
                None => {
                    self.pos = self.s.len();
                    return Ok(());
                }
            }
            if self.eat("<!--") {
                self.skip_past("-->")?;
            } else if self.eat("<?") {
                self.skip_past("?>")?;
            } else if self.rest().starts_with("<!") {
                return self.err("a declaration or CDATA, which these files don't have");
            } else {
                return Ok(());
            }
        }
    }

    fn element(&mut self) -> Result<Element, String> {
        if !self.eat("<") {
            return self.err("< expected");
        }
        let name = self.name()?;
        let mut attrs = Vec::new();
        loop {
            self.skip_space();
            if self.eat("/>") {
                return Ok(Element {
                    name,
                    attrs,
                    children: Vec::new(),
                });
            }
            if self.eat(">") {
                break;
            }
            let attr = self.name()?;
            self.skip_space();
            if !self.eat("=") {
                return self.err("= expected");
            }
            self.skip_space();
            let quote = match self.rest().chars().next() {
                Some(q @ ('"' | '\'')) => q,
                _ => return self.err("a quoted value expected"),
            };
            self.pos += 1;
            let value = self.text_until(quote)?;
            self.pos += 1;
            if attrs.iter().any(|(n, _)| *n == attr) {
                return self.err(&format!("attribute {attr} twice"));
            }
            attrs.push((attr, value));
        }
        let mut children = Vec::new();
        loop {
            self.skip_misc()?;
            if self.rest().is_empty() {
                return self.err(&format!("<{name}> not closed"));
            }
            if self.eat("</") {
                let end = self.name()?;
                self.skip_space();
                if end != name || !self.eat(">") {
                    return self.err(&format!("</{name}> expected"));
                }
                return Ok(Element {
                    name,
                    attrs,
                    children,
                });
            }
            children.push(self.element()?);
        }
    }
}

/// `raw` with the predefined entities and character references replaced.
fn unescape(raw: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find(';') else {
            return Err("& without ;".into());
        };
        let entity = &rest[i + 1..i + j];
        let c = match entity {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse().ok()
                } else {
                    None
                };
                code.and_then(char::from_u32)
                    .ok_or(format!("unknown entity &{entity};"))?
            }
        };
        out.push(c);
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The document's root element.
pub fn parse_xml(text: &str) -> Result<Element, String> {
    let mut p = Parser { s: text, pos: 0 };
    p.skip_misc()?;
    let root = p.element()?;
    p.skip_misc()?;
    if !p.rest().trim().is_empty() {
        return p.err("text after the root element");
    }
    Ok(root)
}

// ---- The protocol ----

/// An argument's type, as the XML names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgType {
    Int,
    Uint,
    Fixed,
    String,
    Object,
    NewId,
    Array,
    Fd,
}

impl ArgType {
    fn parse(s: &str) -> Result<ArgType, String> {
        Ok(match s {
            "int" => ArgType::Int,
            "uint" => ArgType::Uint,
            "fixed" => ArgType::Fixed,
            "string" => ArgType::String,
            "object" => ArgType::Object,
            "new_id" => ArgType::NewId,
            "array" => ArgType::Array,
            "fd" => ArgType::Fd,
            _ => return Err(format!("unknown argument type {s}")),
        })
    }

    /// Its letter in a signature.
    fn letter(self) -> char {
        match self {
            ArgType::Int => 'i',
            ArgType::Uint => 'u',
            ArgType::Fixed => 'f',
            ArgType::String => 's',
            ArgType::Object => 'o',
            ArgType::NewId => 'n',
            ArgType::Array => 'a',
            ArgType::Fd => 'h',
        }
    }

    fn word(self) -> &'static str {
        match self {
            ArgType::Int => "int",
            ArgType::Uint => "uint",
            ArgType::Fixed => "fixed",
            ArgType::String => "string",
            ArgType::Object => "object",
            ArgType::NewId => "new_id",
            ArgType::Array => "array",
            ArgType::Fd => "fd",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    pub name: String,
    pub ty: ArgType,
    /// The interface of an object or a new_id; None for one of any.
    pub interface: Option<String>,
    pub nullable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub name: String,
    pub since: u32,
    pub destructor: bool,
    pub args: Vec<Arg>,
}

impl Message {
    /// libwayland's signature (the module's documentation).
    pub fn signature(&self) -> String {
        let mut s = String::new();
        if self.since > 1 {
            s.push_str(&self.since.to_string());
        }
        for a in &self.args {
            if a.nullable {
                s.push('?');
            }
            if a.ty == ArgType::NewId && a.interface.is_none() {
                s.push_str("su");
            }
            s.push(a.ty.letter());
        }
        s
    }

    /// Its types: per letter of the signature, the interface or None.
    pub fn types(&self) -> Vec<Option<&str>> {
        let mut t = Vec::new();
        for a in &self.args {
            if a.ty == ArgType::NewId && a.interface.is_none() {
                t.extend([None, None]);
            }
            t.push(match a.ty {
                ArgType::Object | ArgType::NewId => a.interface.as_deref(),
                _ => None,
            });
        }
        t
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Enum {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Interface {
    pub name: String,
    pub version: u32,
    pub requests: Vec<Message>,
    pub events: Vec<Message>,
    pub enums: Vec<Enum>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Protocol {
    pub name: String,
    pub interfaces: Vec<Interface>,
}

fn need<'a>(e: &'a Element, attr: &str) -> Result<&'a str, String> {
    e.attr(attr).ok_or(format!("<{}> without {attr}", e.name))
}

fn number(e: &Element, attr: &str, default: u32) -> Result<u32, String> {
    match e.attr(attr) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|_| format!("<{}> {attr}=\"{v}\"", e.name)),
    }
}

fn message(e: &Element) -> Result<Message, String> {
    let mut args = Vec::new();
    for a in e.each("arg") {
        args.push(Arg {
            name: need(a, "name")?.to_string(),
            ty: ArgType::parse(need(a, "type")?)?,
            interface: a.attr("interface").map(str::to_string),
            nullable: a.attr("allow-null") == Some("true"),
        });
    }
    Ok(Message {
        name: need(e, "name")?.to_string(),
        since: number(e, "since", 1)?,
        destructor: e.attr("type") == Some("destructor"),
        args,
    })
}

/// The protocol a document describes.
pub fn protocol(root: &Element) -> Result<Protocol, String> {
    if root.name != "protocol" {
        return Err(format!("<{}>, not <protocol>", root.name));
    }
    let mut interfaces = Vec::new();
    for i in root.each("interface") {
        let name = need(i, "name")?.to_string();
        let mut enums = Vec::new();
        for e in i.each("enum") {
            let mut entries = Vec::new();
            for entry in e.each("entry") {
                entries.push((
                    need(entry, "name")?.to_string(),
                    need(entry, "value")?.to_string(),
                ));
            }
            enums.push(Enum {
                name: need(e, "name")?.to_string(),
                entries,
            });
        }
        let iface = Interface {
            version: number(i, "version", 1)?,
            requests: i.each("request").map(message).collect::<Result<_, _>>()?,
            events: i.each("event").map(message).collect::<Result<_, _>>()?,
            enums,
            name,
        };
        // One header holds the requests' and the events' opcodes: their
        // names must differ.
        for r in &iface.requests {
            if iface.events.iter().any(|e| e.name == r.name) {
                return Err(format!("{}: a request and an event named {}", iface.name, r.name));
            }
        }
        interfaces.push(iface);
    }
    Ok(Protocol {
        name: need(root, "name")?.to_string(),
        interfaces,
    })
}

/// The protocols of [`XML`], read under `root`.
pub fn protocols(root: &Path) -> Result<Vec<Protocol>, String> {
    XML.iter()
        .map(|file| {
            let path = root.join(file);
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            parse_xml(&text)
                .and_then(|x| protocol(&x))
                .map_err(|e| format!("{}: {e}", path.display()))
        })
        .collect()
}

// ---- The C ----

const PREAMBLE: &str = "/* Generated by `cargo xtask kernel` (crates/xtask/src/wayland.rs) from
 * protocols/wayland.xml (Wayland 1.23.0), protocols/xdg-shell.xml
 * (wayland-protocols 1.38) and wlroots' protocols/wlr-layer-shell-unstable-v1.xml
 * and wlr-foreign-toplevel-management-unstable-v1.xml, whose licences their
 * copyright elements give (MIT, and an MIT-style one): do not edit. */
";

fn upper(s: &str) -> String {
    s.to_ascii_uppercase()
}

/// An argument for a comment: `object wl_buffer buffer (or null)`.
fn describe(a: &Arg) -> String {
    let mut s = a.ty.word().to_string();
    if let Some(i) = &a.interface {
        s.push(' ');
        s.push_str(i);
    } else if a.ty == ArgType::NewId {
        s.push_str(" (string interface, uint version)");
    }
    s.push(' ');
    s.push_str(&a.name);
    if a.nullable {
        s.push_str(" (or null)");
    }
    s
}

fn header(protocols: &[Protocol]) -> String {
    let mut h = String::from(PREAMBLE);
    h.push_str(
        "#ifndef CAMELOT_GUI_PROTOCOL_H
#define CAMELOT_GUI_PROTOCOL_H

/* A request or an event: its name, its arguments' signature (libwayland's:
 * the version it came in when past 1, then per argument `?` if it may be
 * null and i int, u uint, f fixed, s string, o object, n new_id, a array,
 * h fd; an untyped new_id is \"sun\") and, per letter, the interface of an
 * object or a new_id, or NULL. As libwayland's wayland-util.h has them. */
struct wl_message {
    const char *name;
    const char *signature;
    const struct wl_interface *const *types;
};

/* An interface: its name, its version, its requests (methods) and events,
 * each indexed by its opcode. */
struct wl_interface {
    const char *name;
    int version;
    int method_count;
    const struct wl_message *methods;
    int event_count;
    const struct wl_message *events;
};
",
    );
    for p in protocols {
        h.push_str(&format!("\n/* ==== {} ==== */\n", p.name));
        for i in &p.interfaces {
            let up = upper(&i.name);
            h.push_str(&format!(
                "\n/* ---- {} (version {}) ---- */\n\nextern const struct wl_interface {}_interface;\n",
                i.name, i.version, i.name
            ));
            for (kind, messages) in [("", &i.requests), ("event: ", &i.events)] {
                for (op, m) in messages.iter().enumerate() {
                    let mut what: Vec<String> = m.args.iter().map(describe).collect();
                    if m.destructor {
                        what.insert(0, "destructor".into());
                    }
                    let comment = match (kind.is_empty(), what.is_empty()) {
                        (true, true) => String::new(),
                        (false, true) => " /* event */".to_string(),
                        _ => format!(" /* {kind}{} */", what.join(", ")),
                    };
                    h.push_str(&format!("#define {up}_{} {op}{comment}\n", upper(&m.name)));
                }
            }
            for m in i.requests.iter().chain(&i.events) {
                h.push_str(&format!(
                    "#define {up}_{}_SINCE_VERSION {}\n",
                    upper(&m.name),
                    m.since
                ));
            }
            for e in &i.enums {
                for (name, value) in &e.entries {
                    h.push_str(&format!(
                        "#define {up}_{}_{} {value}\n",
                        upper(&e.name),
                        upper(name)
                    ));
                }
            }
        }
    }
    h.push_str("\n#endif\n");
    h
}

fn source(protocols: &[Protocol]) -> String {
    let mut c = String::from(PREAMBLE);
    c.push_str("#include \"protocol.h\"\n\n#define NULL ((void *)0)\n\n");
    // Every message's types, one after another, as wayland-scanner lays
    // them out; each message points at its first.
    let mut types: Vec<Option<&str>> = Vec::new();
    let mut starts = Vec::new();
    for p in protocols {
        for i in &p.interfaces {
            for m in i.requests.iter().chain(&i.events) {
                starts.push(types.len());
                types.extend(m.types());
            }
        }
    }
    c.push_str("static const struct wl_interface *const types[] = {\n");
    for t in &types {
        match t {
            Some(i) => c.push_str(&format!("    &{i}_interface,\n")),
            None => c.push_str("    NULL,\n"),
        }
    }
    c.push_str("};\n");
    let mut k = 0;
    for p in protocols {
        for i in &p.interfaces {
            for (kind, messages) in [("requests", &i.requests), ("events", &i.events)] {
                if messages.is_empty() {
                    continue;
                }
                c.push_str(&format!(
                    "\nstatic const struct wl_message {}_{kind}[] = {{\n",
                    i.name
                ));
                for m in messages.iter() {
                    c.push_str(&format!(
                        "    {{\"{}\", \"{}\", &types[{}]}},\n",
                        m.name,
                        m.signature(),
                        starts[k]
                    ));
                    k += 1;
                }
                c.push_str("};\n");
            }
            let table = |kind: &str, n: usize| {
                if n == 0 {
                    "0, NULL".to_string()
                } else {
                    format!("{n}, {}_{kind}", i.name)
                }
            };
            c.push_str(&format!(
                "\nconst struct wl_interface {0}_interface = {{\"{0}\", {1}, {2}, {3}}};\n",
                i.name,
                i.version,
                table("requests", i.requests.len()),
                table("events", i.events.len())
            ));
        }
    }
    c
}

/// The generated files and their text.
pub fn files(root: &Path) -> Result<Vec<(&'static str, String)>, String> {
    let protocols = protocols(root)?;
    Ok(vec![(HEADER, header(&protocols)), (SOURCE, source(&protocols))])
}

/// Writes [`files`] where they differ from what is there; returns those it
/// wrote.
pub fn write(root: &Path) -> Result<Vec<&'static str>, String> {
    let mut wrote = Vec::new();
    for (file, text) in files(root)? {
        let path = root.join(file);
        if std::fs::read_to_string(&path).is_ok_and(|old| old.replace('\r', "") == text) {
            continue;
        }
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        wrote.push(file);
    }
    Ok(wrote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_elements_attributes_comments_and_entities() {
        let x = parse_xml(
            "<?xml version=\"1.0\"?>\n<!-- a comment <with> tags -->\n<a x=\"1 &lt;2&gt; &amp;&quot;\" y='&#65;&#x42;'>\
             text<b/><!-- c --><c z=\"\">more</c></a>\n",
        )
        .unwrap();
        assert_eq!(x.name, "a");
        assert_eq!(x.attr("x"), Some("1 <2> &\""));
        assert_eq!(x.attr("y"), Some("AB"));
        assert_eq!(x.children.len(), 2);
        assert_eq!(x.children[1].attr("z"), Some(""));
        for bad in [
            "<a>",
            "<a></b>",
            "<a x=1/>",
            "<a x=\"&nope;\"/>",
            "<a/><b/>",
            "<a x=\"1\" x=\"2\"/>",
        ] {
            assert!(parse_xml(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn signatures_are_libwaylands() {
        let x = parse_xml(
            "<protocol name=\"p\"><interface name=\"i\" version=\"4\">\
             <request name=\"bind\"><arg name=\"name\" type=\"uint\"/><arg name=\"id\" type=\"new_id\"/></request>\
             <request name=\"attach\" since=\"4\"><arg name=\"b\" type=\"object\" interface=\"j\" allow-null=\"true\"/>\
             <arg name=\"x\" type=\"int\"/></request>\
             <event name=\"e\"><arg name=\"f\" type=\"fd\"/><arg name=\"a\" type=\"array\"/>\
             <arg name=\"v\" type=\"fixed\"/><arg name=\"s\" type=\"string\"/></event>\
             <enum name=\"k\"><entry name=\"one\" value=\"0x1\"/></enum></interface></protocol>",
        )
        .unwrap();
        let p = protocol(&x).unwrap();
        let i = &p.interfaces[0];
        assert_eq!(i.requests[0].signature(), "usun");
        assert_eq!(i.requests[0].types(), [None, None, None, None]);
        assert_eq!(i.requests[1].signature(), "4?oi");
        assert_eq!(i.requests[1].types(), [Some("j"), None]);
        assert_eq!(i.events[0].signature(), "hafs");
        assert_eq!(i.enums[0].entries, [("one".to_string(), "0x1".to_string())]);
    }

    #[test]
    fn a_request_and_an_event_of_one_name_are_refused() {
        let x = parse_xml(
            "<protocol name=\"p\"><interface name=\"i\" version=\"1\"><request name=\"a\"/>\
             <event name=\"a\"/></interface></protocol>",
        )
        .unwrap();
        assert!(protocol(&x)
            .unwrap_err()
            .contains("a request and an event named a"));
    }

    #[test]
    fn the_protocols_have_what_the_server_implements() {
        let ps = protocols(&crate::repo_root()).unwrap();
        let find = |name: &str| {
            ps.iter()
                .flat_map(|p| &p.interfaces)
                .find(|i| i.name == name)
                .unwrap()
        };
        assert_eq!(find("wl_compositor").version, 6);
        let surface = find("wl_surface");
        assert_eq!(surface.requests[1].name, "attach");
        assert_eq!(surface.requests[1].signature(), "?oii");
        let bind = &find("wl_registry").requests[0];
        assert_eq!(bind.signature(), "usun");
        assert_eq!(find("xdg_toplevel").requests[5].name, "move");
    }
}
