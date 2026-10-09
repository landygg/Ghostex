fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = gxserver::cli::run_hook_command(&args) {
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    // Before the runtime starts any thread: PATH is process-wide state.
    gxserver::prepare_process_environment();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("start the gxserver runtime");
    runtime.block_on(async {
        if let Err(error) = gxserver::cli::run(args).await {
            eprintln!("{error}");
            std::process::exit(1);
        }
    });
}
