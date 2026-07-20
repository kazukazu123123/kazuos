#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

use core::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    println!("threadtest: spawning");
    let arg = 42u64;
    let handle = thread_create(move || {
        for i in 0..3 {
            COUNTER.fetch_add(1, Ordering::SeqCst);
            println!("worker arg={} tick={}", arg, i);
            sys_sleep(50);
        }
        println!("worker done");
    });
    let Some(handle) = handle else {
        println!("threadtest: spawn failed");
        sys_exit(1);
    };
    println!("threadtest: spawned tid={}", handle.tid());
    handle.join();
    println!("threadtest: joined counter={}", COUNTER.load(Ordering::SeqCst));
    sys_exit(0);
}
