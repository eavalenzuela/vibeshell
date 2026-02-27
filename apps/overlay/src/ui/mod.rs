use std::collections::HashMap;
use std::rc::Rc;

use common::contracts::{CanvasState, Cluster, ClusterId, Window};
use gtk::prelude::*;
use gtk4 as gtk;

pub fn render_clusters(list: &gtk::Box, state: &CanvasState, on_activate: Rc<dyn Fn(ClusterId)>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let by_id: HashMap<_, _> = state
        .windows
        .iter()
        .map(|window| (window.id, window))
        .collect();

    for cluster in &state.clusters {
        list.append(&cluster_card(cluster, &by_id, Rc::clone(&on_activate)));
    }
}

fn cluster_card(
    cluster: &Cluster,
    windows_by_id: &HashMap<u64, &Window>,
    on_activate: Rc<dyn Fn(ClusterId)>,
) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();
    card.add_css_class("card");

    let header = gtk::Label::new(Some(&format!(
        "{} ({})",
        cluster.name,
        cluster.windows.len()
    )));
    header.set_xalign(0.0);
    header.add_css_class("title-4");
    card.append(&header);

    let window_list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();

    for window_id in &cluster.windows {
        let text = if let Some(window) = windows_by_id.get(window_id) {
            let app_id = window.app_id.as_deref().unwrap_or("unknown-app");
            let title = if window.title.trim().is_empty() {
                "untitled"
            } else {
                window.title.as_str()
            };
            format!("• {} — {}", title, app_id)
        } else {
            format!("• closed window ({window_id})")
        };

        let row = gtk::Label::new(Some(&text));
        row.set_xalign(0.0);
        row.set_wrap(true);
        window_list.append(&row);
    }

    if cluster.windows.is_empty() {
        let empty = gtk::Label::new(Some("• no windows"));
        empty.set_xalign(0.0);
        window_list.append(&empty);
    }

    card.append(&window_list);

    let activate_button = gtk::Button::with_label("Activate");
    let cluster_id = cluster.id;
    activate_button.connect_clicked(move |_| on_activate(cluster_id));
    card.append(&activate_button);

    card
}
