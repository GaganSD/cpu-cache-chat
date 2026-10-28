use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct Pair(Child, Child, std::path::PathBuf);

impl Drop for Pair {
    fn drop(&mut self) {
        for child in [&mut self.0, &mut self.1] {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.2);
    }
}

#[test]
fn connected_quit_succeeds_for_twenty_pairs() {
    for iteration in 0..20 {
        let dir = std::env::temp_dir().join(format!("ccc-quit-{}-{iteration}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let path = dir.join("link");
        let spawn = |role| {
            Command::new(env!("CARGO_BIN_EXE_cpu-cache-chat"))
                .args([
                    "game",
                    "--role",
                    role,
                    "--channel",
                    "cpu",
                    "--sim",
                    "--plain",
                    "--sim-path",
                ])
                .arg(&path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap()
        };
        let mut pair = Pair(spawn("a"), spawn("b"), dir);
        let (tx, rx) = mpsc::channel();
        for child in [&mut pair.0, &mut pair.1] {
            let stdout = child.stdout.take().unwrap();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut reported = false;
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if !reported && line.contains("link: connected") {
                        let _ = tx.send(());
                        reported = true;
                    }
                }
            });
        }
        rx.recv_timeout(Duration::from_secs(10))
            .expect("first peer did not connect");
        rx.recv_timeout(Duration::from_secs(10))
            .expect("second peer did not connect");
        for child in [&mut pair.0, &mut pair.1] {
            child.stdin.as_mut().unwrap().write_all(b"/q\n").unwrap();
        }
        assert!(
            pair.0.wait().unwrap().success(),
            "role a, iteration {iteration}"
        );
        assert!(
            pair.1.wait().unwrap().success(),
            "role b, iteration {iteration}"
        );
    }
}
