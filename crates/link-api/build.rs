//! Generates the `org.andromeda.Link1` client from `api/org.andromeda.Link1.xml`, the API's one source.

const SOURCE: &str = "../../api/org.andromeda.Link1.xml";
const INTERFACES: [&str; 2] = ["org.andromeda.Link1", "org.andromeda.Link1.QuickShare"];
/// Kept equal to `SERVICE` and `PATH` in `src/lib.rs`.
const SERVICE: &str = "org.andromeda.Link1";
const PATH: &str = "/org/andromeda/Link1";

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> Result {
    println!("cargo::rerun-if-changed={SOURCE}");
    let mut xml = std::fs::read_to_string(SOURCE)?;
    // A named pipe passes no descriptors: the Windows client leaves out the members that return one.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        xml = without_fd_methods(&xml)?;
    }
    let node = zbus_xml::Node::from_reader(xml.as_bytes())?;
    let service = zbus::names::BusName::from_static_str(SERVICE)?;
    let path = zbus::ObjectPath::from_static_str(PATH)?;
    let mut code = String::new();
    for name in INTERFACES {
        let interface = node.interfaces().iter().find(|interface| interface.name() == name).ok_or(name)?;
        code += &zbus_xmlgen::CodeGenerator::new()
            .with_service(Some(&service))
            .with_path(Some(&path))
            .interface_code(interface)?;
    }
    std::fs::write(std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("link1.rs"), code)?;
    Ok(())
}

/// The XML without the methods and signals that have a descriptor (`h`) in any argument.
fn without_fd_methods(xml: &str) -> Result<String> {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = ["<method ", "<signal "].iter().filter_map(|tag| rest.find(tag)).min() {
        let open = start + rest[start..].find('>').ok_or("an unclosed tag")?;
        let end = if rest[..open].ends_with('/') {
            open + 1
        } else {
            let close = if rest[start..].starts_with("<method ") { "</method>" } else { "</signal>" };
            open + rest[open..].find(close).ok_or("an unclosed element")? + close.len()
        };
        out += &rest[..start];
        let element = &rest[start..end];
        if !element.split(r#"type=""#).skip(1).any(|value| value.split('"').next().is_some_and(|t| t.contains('h'))) {
            out += element;
        }
        rest = &rest[end..];
    }
    Ok(out + rest)
}
