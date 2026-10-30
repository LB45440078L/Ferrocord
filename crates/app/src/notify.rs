//! Native desktop notifications.

use ferrocord_core::Notification;

pub fn show(n: &Notification, show_content: bool, sound: bool) {
    let title = n.title.clone();
    let body = if show_content { truncate(&n.body, 200) } else { "New message".to_string() };
    std::thread::spawn(move || {
        let mut notif = notify_rust::Notification::new();
        notif.summary(&title).body(&body).appname("Ferrocord");
        #[cfg(windows)]
        {
            if !sound {
                notif.sound_name("silent");
            }
        }
        #[cfg(not(windows))]
        {
            let _ = sound;
        }
        if let Err(e) = notif.show() {
            tracing::debug!(error = %e, "notification failed");
        }
    });
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}
