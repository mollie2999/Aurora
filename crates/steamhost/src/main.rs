//! Host Valve's own steamclient (`linux64/steamclient.so` on Linux,
//! `steamclient64.dll` on Windows) as the global Steam instance, with no
//! steamui and no steamwebhelper.
//!
//! The installed library supplies the engine. Profile forwarding on verified
//! Windows builds redirects one dispatch call in this process's memory.
//!
//! `steamhost probe <steam-root>`        become the global instance and name
//!                                       the interfaces the getters return
//! `steamhost client-test <steam-root>`  connect to it like a game would
//! `steamhost login <steam-root>`        sign in (account, refresh token and
//!                                       SteamID on stdin), then hold it
//! `steamhost serve <steam-root>`        sign in, then serve games until
//!                                       stdin closes

#[cfg(any(target_os = "linux", windows))]
mod engine;
#[cfg(any(target_os = "linux", windows))]
mod sys;

#[cfg(any(target_os = "linux", windows))]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    sys::ensure_quiet_flags(&args);
    let (Some(cmd), Some(root)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: steamhost probe|client-test|login|serve <steam-root>");
        sys::quit(2);
    };
    let root = std::path::Path::new(root);
    let code = match cmd.as_str() {
        "probe" => engine::probe(root),
        "client-test" => engine::client_test(root),
        "login" => engine::login(root),
        "serve" => engine::serve(root),
        _ => Err(format!("unknown command {cmd}")),
    };
    let status = match code {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("steamhost: {e}");
            1
        }
    };
    // Valve's engine threads are still running: leave without them.
    use std::io::Write;
    let _ = std::io::stdout().flush();
    sys::quit(status)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn main() {
    eprintln!("steamhost only runs on Linux and Windows");
    std::process::exit(1);
}
