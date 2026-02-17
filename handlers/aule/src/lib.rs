use gtk4::prelude::*;
use gtk4::{Box, Orientation, Button, Widget};
use async_channel::Sender;
use elessar::gneiss_pal::Event;

pub fn create_view(tx: Sender<Event>) -> Widget {
    let aule_box = Box::new(Orientation::Vertical, 10);
    aule_box.set_margin_top(20);

    let ignite_btn = Button::with_label("Ignite");
    ignite_btn.set_icon_name("applications-engineering-symbolic");
    ignite_btn.add_css_class("suggested-action");

    let tx_clone = tx.clone();
    ignite_btn.connect_clicked(move |_| {
        let _ = tx_clone.send_blocking(Event::AuleIgnite);
    });

    aule_box.append(&ignite_btn);
    aule_box.upcast::<Widget>()
}
