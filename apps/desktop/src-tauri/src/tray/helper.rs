//! Hosts the native status item in a windowless AppKit process on macOS 27.
//! The two private pipes carry menu snapshots and user actions. Closing stdin
//! when the desktop exits also removes the helper and its status item.

use crate::{tray::menu::Entry, tray::speed, tray::state::TrayMenuData};
use dispatch2::DispatchQueue;
use objc2::{sel, ClassType, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSStatusItem};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;

pub(crate) const ARG: &str = "--crossbow-native-tray";

#[derive(Deserialize, Serialize)]
pub(crate) enum Command {
    Menu(Vec<Entry>, TrayMenuData),
    Nodes(TrayMenuData),
    Speed(String),
}

#[derive(Deserialize, Serialize)]
pub(crate) enum Event {
    Action(String),
    RefreshNodes,
    RefreshMenu,
}

pub(crate) struct Helper(Arc<Mutex<Option<Sender<Command>>>>);

impl Helper {
    pub(crate) fn send(&self, command: Command) -> Result<(), Command> {
        match self.0.lock().unwrap().as_ref() {
            Some(tx) => tx.send(command).map_err(|error| error.0),
            None => Err(command),
        }
    }
}

pub(crate) fn start(handle: &AppHandle) -> io::Result<Option<Helper>> {
    if NSStatusItem::class()
        .instance_method(sel!(setExpandedInterfaceDelegate:))
        .is_none()
    {
        return Ok(None);
    }
    let mut child = ProcessCommand::new(std::env::current_exe()?)
        .arg(ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().expect("piped helper stdin");
    let output = child.stdout.take().expect("piped helper stdout");
    let (tx, rx) = mpsc::channel();
    let sender = Arc::new(Mutex::new(Some(tx)));
    let sender_on_exit = sender.clone();
    std::thread::spawn(move || {
        for command in rx {
            if write_message(&mut input, &command).is_err() {
                break;
            }
        }
    });
    let handle = handle.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if let Ok(event) = serde_json::from_str(&line) {
                let app = handle.clone();
                let _ = handle.run_on_main_thread(move || dispatch(&app, event));
            }
        }
        let _ = child.wait();
        sender_on_exit.lock().unwrap().take();
        let app = handle.clone();
        let _ = handle.run_on_main_thread(move || speed::helper_stopped(&app));
    });
    Ok(Some(Helper(sender)))
}

pub(crate) fn dispatch(handle: &AppHandle, event: Event) {
    match event {
        Event::Action(id) => crate::handle_menu(handle, &id),
        Event::RefreshNodes => crate::refresh_tray_nodes(handle, true),
        Event::RefreshMenu => speed::refresh_menu(handle),
    }
}

pub(crate) fn run() -> io::Result<()> {
    let mtm = MainThreadMarker::new().expect("AppKit runs on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    speed::install_native(|event| {
        if write_message(&mut io::stdout().lock(), &event).is_err() {
            std::process::exit(0);
        }
    })?;
    std::thread::spawn(|| {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if let Ok(command) = serde_json::from_str(&line) {
                DispatchQueue::main().exec_async(move || speed::apply_command(command));
            }
        }
        std::process::exit(0);
    });
    app.run();
    Ok(())
}

fn write_message(writer: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, message)?;
    writer.write_all(b"\n")?;
    writer.flush()
}
