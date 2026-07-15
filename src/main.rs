//! Throwaway XDG Global Shortcuts probe for issue 23.

use ashpd::desktop::{
    global_shortcuts::{
        BindShortcutsOptions, GlobalShortcuts, ListShortcutsOptions, NewShortcut, Shortcut,
    },
    CreateSessionOptions,
};
use ashpd::{zbus, zvariant::OwnedValue};
use futures_util::StreamExt;
use std::{
    collections::HashMap,
    env,
    error::Error,
    fs,
    path::PathBuf,
    process::{Command, Stdio},
};

const APP_ID: &str = "app.openfrag.PortalProbe";
const SHORTCUT_ID: &str = "manual_flag";
const PREFERRED_TRIGGER: &str = "CTRL+SHIFT+F10";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    match env::args().nth(1).as_deref() {
        None | Some("inspect") => inspect().await,
        Some("live") => {
            let cycles = env::args()
                .nth(2)
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(3);
            live(cycles).await
        }
        Some("cleanup") => cleanup(),
        Some(other) => {
            Err(format!("unknown command: {other}; use inspect, live [cycles], or cleanup").into())
        }
    }
}

async fn inspect() -> Result<(), Box<dyn Error>> {
    let portal = registered_portal().await?;
    println!("global_shortcuts_interface=true");
    println!("global_shortcuts_version={}", portal.version());
    Ok(())
}

async fn live(target_cycles: u32) -> Result<(), Box<dyn Error>> {
    if target_cycles == 0 {
        return Err("cycles must be at least 1".into());
    }

    let portal = registered_portal().await?;
    println!("global_shortcuts_version={}", portal.version());

    let mut activated = portal.receive_activated().await?;
    let mut deactivated = portal.receive_deactivated().await?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;

    let listed = portal
        .list_shortcuts(&session, ListShortcutsOptions::default())
        .await?
        .response()?;

    let restored = listed
        .shortcuts()
        .iter()
        .find(|shortcut| shortcut.id() == SHORTCUT_ID);
    let bound = if let Some(shortcut) = restored {
        println!("restored_binding=true");
        print_shortcut(shortcut);
        true
    } else {
        println!("restored_binding=false");
        println!("portal_dialog_expected=true");
        let proposed = NewShortcut::new(SHORTCUT_ID, "Save a Manual Flag")
            .preferred_trigger(PREFERRED_TRIGGER);
        let response = portal
            .bind_shortcuts(&session, &[proposed], None, BindShortcutsOptions::default())
            .await?
            .response()?;
        if let Some(shortcut) = response
            .shortcuts()
            .iter()
            .find(|shortcut| shortcut.id() == SHORTCUT_ID)
        {
            print_shortcut(shortcut);
            true
        } else {
            false
        }
    };

    if !bound {
        session.close().await?;
        println!("binding_accepted=false");
        return Err("the portal returned no Manual Flag binding".into());
    }

    println!("binding_accepted=true");
    println!("test_instructions=press and hold the chosen shortcut, then release it; repeat while changing focus or locking the desktop as needed");
    println!("target_cycles={target_cycles}");

    let mut is_pressed = false;
    let mut activations = 0_u32;
    let mut deactivations = 0_u32;
    let mut duplicate_activations = 0_u32;
    let mut stray_deactivations = 0_u32;

    while deactivations < target_cycles {
        tokio::select! {
            event = activated.next() => {
                let event = event.ok_or("activation signal stream ended")?;
                if event.shortcut_id() != SHORTCUT_ID {
                    continue;
                }
                if is_pressed {
                    duplicate_activations += 1;
                    println!("duplicate_activation timestamp_ms={}", event.timestamp().as_millis());
                } else {
                    is_pressed = true;
                    activations += 1;
                    println!("activated count={activations} timestamp_ms={}", event.timestamp().as_millis());
                }
            }
            event = deactivated.next() => {
                let event = event.ok_or("deactivation signal stream ended")?;
                if event.shortcut_id() != SHORTCUT_ID {
                    continue;
                }
                if is_pressed {
                    is_pressed = false;
                    deactivations += 1;
                    println!("deactivated count={deactivations} timestamp_ms={}", event.timestamp().as_millis());
                } else {
                    stray_deactivations += 1;
                    println!("stray_deactivation timestamp_ms={}", event.timestamp().as_millis());
                }
            }
            result = tokio::signal::ctrl_c() => {
                result?;
                println!("interrupted=true");
                break;
            }
        }
    }

    session.close().await?;
    println!("session_closed=true");
    println!("activations={activations}");
    println!("deactivations={deactivations}");
    println!("duplicate_activations={duplicate_activations}");
    println!("stray_deactivations={stray_deactivations}");
    println!(
        "exactly_once={}",
        activations == deactivations
            && activations > 0
            && duplicate_activations == 0
            && stray_deactivations == 0
    );
    Ok(())
}

fn print_shortcut(shortcut: &Shortcut) {
    println!("shortcut_id={}", shortcut.id());
    println!("shortcut_description={}", shortcut.description());
    println!("trigger_description={}", shortcut.trigger_description());
}

async fn registered_portal() -> Result<GlobalShortcuts, Box<dyn Error>> {
    let desktop_file = ensure_desktop_file()?;
    println!("desktop_file={}", desktop_file.display());
    let connection = zbus::Connection::session().await?;
    let registry = zbus::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.host.portal.Registry",
    )
    .await?;
    let options = HashMap::<&str, OwnedValue>::new();
    registry.call_method("Register", &(APP_ID, options)).await?;
    println!("registered_app_id={APP_ID}");
    Ok(GlobalShortcuts::with_connection(connection).await?)
}

fn ensure_desktop_file() -> Result<PathBuf, Box<dyn Error>> {
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or("HOME and XDG_DATA_HOME are both unavailable")?;
    let applications = data_home.join("applications");
    fs::create_dir_all(&applications)?;
    let path = applications.join(format!("{APP_ID}.desktop"));
    let executable = env::current_exe()?;
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName=openfrag Portal Probe\nComment=Temporary identity for the openfrag Global Shortcuts prototype\nExec={} live\nNoDisplay=true\nTerminal=true\n",
        executable.display()
    );
    if path.exists() {
        if fs::read_to_string(&path)? != contents {
            return Err(format!(
                "refusing to replace existing desktop file: {}",
                path.display()
            )
            .into());
        }
    } else {
        fs::write(&path, contents)?;
    }
    refresh_desktop_database(&applications)?;
    Ok(path)
}

fn cleanup() -> Result<(), Box<dyn Error>> {
    let desktop_file = ensure_desktop_file()?;
    let status = Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.kde.kglobalaccel",
            "/kglobalaccel",
            "org.kde.KGlobalAccel",
            "unregister",
            "ss",
            APP_ID,
            SHORTCUT_ID,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    println!(
        "kde_binding_removed={}",
        status.is_ok_and(|status| status.success())
    );
    fs::remove_file(&desktop_file)?;
    refresh_desktop_database(
        desktop_file
            .parent()
            .ok_or("desktop file has no parent directory")?,
    )?;
    println!("desktop_file_removed=true");
    Ok(())
}

fn refresh_desktop_database(applications: &std::path::Path) -> Result<(), Box<dyn Error>> {
    match Command::new("update-desktop-database")
        .arg(applications)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("update-desktop-database exited with {status}").into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
