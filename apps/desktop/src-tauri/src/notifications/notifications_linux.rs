//! A generic system reminder; the Rust notification queue remains authoritative.
use dbus::{arg::Variant, blocking::Connection, Message, MessageType};
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";
const TIMEOUT: Duration = Duration::from_millis(300);
const BACKOFF: Duration = Duration::from_secs(5);

#[derive(Clone, PartialEq)]
pub(super) struct Reminder {
    pub ids: Vec<(String, u64)>,
    pub english: bool,
    pub urgent: bool,
}

#[derive(Default)]
struct Pending {
    reminder: Option<Reminder>,
    stop: bool,
    finished: bool,
}

pub(super) struct Notifier(Arc<(Mutex<Pending>, Condvar)>);

impl Notifier {
    pub fn start(
        on_open: impl Fn() + Send + 'static,
        on_dismiss: impl Fn(&[(String, u64)]) + Send + 'static,
    ) -> Self {
        let shared = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker = shared.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("linux-notifications".into())
            .spawn(move || {
                run(&worker, on_open, on_dismiss);
                worker.0.lock().unwrap().finished = true;
                worker.1.notify_all();
            })
        {
            eprintln!("Crossbow system notification worker unavailable: {error}");
            shared.0.lock().unwrap().finished = true;
        }
        Self(shared)
    }

    pub fn update(&self, reminder: Option<Reminder>) {
        self.0 .0.lock().unwrap().reminder = reminder;
        self.0 .1.notify_all();
    }

    /// Exit may wait briefly for withdrawal, never for an unbounded thread join.
    pub fn shutdown(&self) {
        let mut pending = self.0 .0.lock().unwrap();
        pending.stop = true;
        self.0 .1.notify_all();
        let _ = self
            .0
             .1
            .wait_timeout_while(pending, Duration::from_secs(1), |p| !p.finished);
    }
}

struct Daemon {
    connection: Connection,
    owner: String,
    displayed: Option<(u32, Reminder)>,
}

impl Daemon {
    fn connect() -> Result<Self, dbus::Error> {
        let connection = Connection::new_session()?;
        let bus = connection.with_proxy(BUS, BUS_PATH, TIMEOUT);
        // Subscribe before resolving the owner so an owner change cannot be missed.
        for rule in [
            format!("type='signal',sender='{SERVICE}',interface='{SERVICE}',path='{PATH}'"),
            format!("type='signal',sender='{BUS}',interface='{BUS}',path='{BUS_PATH}',member='NameOwnerChanged',arg0='{SERVICE}'"),
        ] {
            let _: () = bus.method_call(BUS, "AddMatch", (rule,))?;
        }
        // A normal call to the well-known name also starts an activatable daemon.
        let _: (String, String, String, String) = connection
            .with_proxy(SERVICE, PATH, TIMEOUT)
            .method_call(SERVICE, "GetServerInformation", ())?;
        let (owner,): (String,) = bus.method_call(BUS, "GetNameOwner", (SERVICE,))?;
        Ok(Self {
            connection,
            owner,
            displayed: None,
        })
    }

    fn notify(&self, reminder: &Reminder) -> Result<u32, dbus::Error> {
        let (title, body, action) = if reminder.english {
            (
                "New Crossbow notifications",
                "Open Crossbow to view details",
                "Open Crossbow",
            )
        } else {
            (
                "Crossbow 有新通知",
                "打开 Crossbow 查看详情",
                "打开 Crossbow",
            )
        };
        let hints = HashMap::from([("urgency", Variant(if reminder.urgent { 2u8 } else { 1u8 }))]);
        let (id,): (u32,) = self
            .connection
            .with_proxy(&self.owner, PATH, TIMEOUT)
            .method_call(
                SERVICE,
                "Notify",
                (
                    "Crossbow",
                    self.displayed.as_ref().map_or(0, |d| d.0),
                    "",
                    title,
                    body,
                    vec!["default", action],
                    hints,
                    8000i32,
                ),
            )?;
        Ok(id)
    }

    fn close(&mut self) -> Result<(), dbus::Error> {
        // Retire the ID before the RPC: its resulting close signal is not a dismissal.
        if let Some((id, _)) = self.displayed.take() {
            self.connection
                .with_proxy(&self.owner, PATH, TIMEOUT)
                .method_call::<(), _, _, _>(SERVICE, "CloseNotification", (id,))?;
        }
        Ok(())
    }

    fn signal(
        &mut self,
        message: Message,
        on_open: &impl Fn(),
        on_dismiss: &impl Fn(&[(String, u64)]),
    ) -> Result<(), dbus::Error> {
        if message.msg_type() != MessageType::Signal {
            return Ok(());
        }
        let sender = message.sender();
        let interface = message.interface();
        let path = message.path();
        let member = message.member();
        if sender.as_deref() == Some(BUS)
            && interface.as_deref() == Some(BUS)
            && path.as_deref() == Some(BUS_PATH)
            && member.as_deref() == Some("NameOwnerChanged")
        {
            if let Ok((name, _, owner)) = message.read3::<String, String, String>() {
                if name == SERVICE && owner != self.owner {
                    return Err(dbus::Error::new_failed("notification daemon changed"));
                }
            }
        }
        if sender.as_deref() != Some(self.owner.as_str())
            || interface.as_deref() != Some(SERVICE)
            || path.as_deref() != Some(PATH)
        {
            return Ok(());
        }
        match member.as_deref() {
            Some("ActionInvoked") => {
                if let Ok((id, action)) = message.read2::<u32, String>() {
                    if action == "default" && self.displayed.as_ref().is_some_and(|d| d.0 == id) {
                        on_open();
                    }
                }
            }
            Some("NotificationClosed") => {
                if let Ok((id, reason)) = message.read2::<u32, u32>() {
                    if self.displayed.as_ref().is_some_and(|d| d.0 == id) {
                        let (_, reminder) = self.displayed.take().unwrap();
                        if reason == 2 {
                            on_dismiss(&reminder.ids);
                        }
                        // Expiry (1), withdrawal (3), and other reasons never change the queue.
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn run(
    shared: &Arc<(Mutex<Pending>, Condvar)>,
    on_open: impl Fn(),
    on_dismiss: impl Fn(&[(String, u64)]),
) {
    let mut daemon: Option<Daemon> = None;
    let mut delivered = None;
    let mut retry = Instant::now();
    let mut logged = false;
    loop {
        let pending = shared.0.lock().unwrap();
        if pending.stop {
            drop(pending);
            if let Some(d) = daemon.as_mut() {
                let _ = d.close();
            }
            break;
        }
        let desired = pending.reminder.clone();
        if daemon.is_none() && (desired == delivered || Instant::now() < retry) {
            let delay = if Instant::now() < retry {
                retry.saturating_duration_since(Instant::now())
            } else {
                Duration::from_secs(60)
            };
            let _ = shared.1.wait_timeout(pending, delay);
            continue;
        }
        drop(pending);
        let result = (|| {
            if desired.is_none() {
                if let Some(d) = daemon.as_mut() {
                    d.close()?;
                }
                delivered = None;
                daemon = None;
                return Ok(());
            }
            if daemon.is_none() {
                daemon = Some(Daemon::connect()?);
                // Re-read the latest state after connecting; shutdown/update may have arrived.
                return Ok(());
            }
            let d = daemon.as_mut().unwrap();
            // Drain queued signals before replacing their ID/reminder association.
            while let Some(message) = d.connection.channel().pop_message() {
                d.signal(message, &on_open, &on_dismiss)?;
            }
            if desired != delivered {
                let reminder = desired.clone().unwrap();
                let id = d.notify(&reminder)?;
                // Signals queued during Notify still belong to the previous display batch.
                // In particular, a late dismissal must not inherit a new attempt's token.
                while let Some(message) = d.connection.channel().pop_message() {
                    d.signal(message, &on_open, &on_dismiss)?;
                }
                d.displayed = Some((id, reminder));
                delivered = desired;
            }
            if let Some(message) = d
                .connection
                .channel()
                .blocking_pop_message(Duration::from_millis(100))?
            {
                d.signal(message, &on_open, &on_dismiss)?;
            }
            Ok::<(), dbus::Error>(())
        })();
        if let Err(error) = result {
            if !logged {
                eprintln!("Crossbow system notifications unavailable; retrying: {error}");
                logged = true;
            }
            if daemon.as_ref().is_some_and(|d| d.displayed.is_some()) {
                delivered = None;
            }
            daemon = None;
            retry = Instant::now() + BACKOFF;
        }
    }
}
