#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {};
}

#[macro_export]
macro_rules! serial_println {
    () => {};
    ($fmt:expr) => {};
    ($fmt:expr, $($arg:tt)*) => {};
}
