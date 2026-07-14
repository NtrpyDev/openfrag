//! Non-invasive probe for issue 23.
//!
//! This command performs only D-Bus Introspect on the portal object. It does
//! not call CreateSession, BindShortcuts, ListShortcuts, or any other portal
//! method that could show UI, register an accelerator, steal focus, or change
//! desktop state. Activation and lifecycle behavior require explicit HITL.

use std::error::Error;
use zbus::blocking::{Connection, Proxy};

const BUS_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const INTROSPECT_IFACE: &str = "org.freedesktop.DBus.Introspectable";

fn main() -> Result<(), Box<dyn Error>> {
    let connection = Connection::session()?;
    let proxy = Proxy::new(
        &connection,
        BUS_NAME,
        PORTAL_PATH,
        INTROSPECT_IFACE,
    )?;
    let xml: String = proxy.call("Introspect", &())?;

    let has_global_shortcuts = xml.contains("org.freedesktop.portal.GlobalShortcuts");
    println!("global_shortcuts_interface={has_global_shortcuts}");
    println!("portal_object_introspection_bytes={}", xml.len());
    Ok(())
}
