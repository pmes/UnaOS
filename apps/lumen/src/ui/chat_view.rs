let adj = scrolled_window.vadjustment();
adj.connect_upper_notify(|adj| {
    let upper = adj.upper();
    let page_size = adj.page_size();
    // Only scroll if the content is actually larger than the viewport
    if upper > page_size {
        adj.set_value(upper - page_size);
    } else {
        adj.set_value(0.0);
    }
});
