#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    crate::log_fatal!(
        "ALLOC ERROR size={} align={}",
        layout.size(),
        layout.align()
    );
    loop {
        crate::util::pause();
    }
}
