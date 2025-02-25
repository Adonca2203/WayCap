use dbus_codegen::{ConnectionType, GenOpts};
use std::{env, error::Error, path::Path};

fn introspect_one(out_dir: &Path, xml_name: &str) -> Result<(), Box<dyn Error>> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let manifest_dir = Path::new(&manifest_dir);
    let gen_opts = GenOpts {
        connectiontype: ConnectionType::Blocking,
        methodtype: None,
        dbuscrate: "::dbus".into(),
        ..Default::default()
    };
    let introspect_path = manifest_dir.join(format!(
        "dbus_introspections/org.freedesktop.portal.{0}.xml",
        xml_name
    ));
    println!("cargo:rerun-if-changed={0}", introspect_path.display());
    let src = dbus_codegen::generate(&std::fs::read_to_string(introspect_path)?, &gen_opts)?;
    std::fs::write(
        out_dir.join(format!("{0}.rs", xml_name.to_lowercase())),
        src,
    )?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = env::var("OUT_DIR")?;
    let out_dir = Path::new(&out_dir);
    introspect_one(out_dir, "Request")?;
    introspect_one(out_dir, "Session")?;
    introspect_one(out_dir, "ScreenCast")?;

    Ok(())
}
