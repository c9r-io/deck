// ---------- follow-up 02, pinned against the code as it was accepted for review ----------

/// R01. A child of the pane's foreground program, in a session of its own and
/// without a terminal, writes on the connection it inherited. The parent is
/// still the foreground program and the same image: nothing about IT says
/// that these bytes are not its own.
#[test]
fn r01_a_child_with_the_inherited_connection_writes_nothing_for_its_parent() {
    let _guard = fresh();
    let server = Server::start("r01");
    let accepting = Accepting::start(&server, SIGNAL);
    let base = server.base.to_str().unwrap().to_string();
    build_peer(&server.base.join("observer"), "parent");
    let frame = |body: serde_json::Value| format!("'{}'", frame_line(body));
    let working = frame(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    let ending = frame(serde_json::json!({"seq": 2, "run": 1, "state": "turn-done"}));
    server.type_line(
        "deck-a",
        &format!(
            "{base}/observer {base}/status.sock {base}/log '{hello}' {working} @mark:{base}/before \
             @wait:{base}/go @child {ending} @mark:{base}/after @sleep",
            hello = server.hello("deck-a"),
        ),
    );
    let there = |name: &str| server.base.join(name).exists();
    let log = || std::fs::read_to_string(server.base.join("log")).unwrap_or_default();
    until("the parent's run", || there("before"));
    assert!(log().contains("\"ack\":1"), "{}", log());
    assert_eq!(server.state("deck-a"), Some("working"));
    std::fs::write(server.base.join("go"), "").unwrap();
    until("the answer the child read", || there("after"));
    assert_eq!(
        server.fact("deck-a", "#{pane_current_command}"),
        "observer",
        "the parent is still the pane's foreground program"
    );
    assert!(
        !log().contains("\"ack\":2"),
        "the child's frame was acknowledged: {}",
        log()
    );
    assert_ne!(
        server.state("deck-a"),
        Some("turn-done"),
        "an ending nobody but a background child wrote"
    );
    drop(accepting);
}

/// R02. A program writes a hello and a frame and then executes another image
/// in its pid — all before Deck has read a byte. Deck then reads the hello,
/// takes the peer and starts watching it: what it watches is the NEW image,
/// which said none of this.
#[test]
fn r02_what_an_earlier_image_wrote_before_the_watch_is_not_the_new_image_s() {
    let _guard = fresh();
    let server = Server::start("r02");
    let base = server.base.to_str().unwrap().to_string();
    build_peer(&server.base.join("observer"), "before");
    build_peer(&server.base.join("other"), "after");
    let listener =
        agent_status::listen_at(&server.base.join("status.sock")).expect("a status socket");
    let reasons: Reasons = Arc::new(Mutex::new(Vec::new()));
    let runner = server.runner();
    let service = {
        let report = reasons.clone();
        Service::start_in(
            Arc::new(move |reason| report.lock_or_recover().push(reason)),
            World {
                process: Box::new(crate::procinfo::process),
                listing: Box::new(move || crate::tmux::list_panes_with(&runner).ok()),
                enabled: Box::new(|| SIGNAL),
                socket: "t".into(),
            },
            Bounds::LIVE,
        )
    };
    let working = frame_line(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    server.type_line(
        "deck-a",
        &format!(
            "{base}/observer {base}/status.sock {base}/log '@send:{hello}' '@send:{working}' \
             @exec:{base}/other @mark:{base}/after @sleep",
            hello = server.hello("deck-a"),
        ),
    );
    until("the new image", || server.base.join("after").exists());
    assert_eq!(server.fact("deck-a", "#{pane_current_command}"), "other");
    // only now does Deck accept the connection and read what waits in it
    let (stream, _) = listener.accept().expect("the connection that was waiting");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut waiting = Vec::new();
    let mut chunk = [0u8; 2048];
    while waiting.iter().filter(|byte| **byte == b'\n').count() < 2 {
        let read = std::io::Read::read(&mut &stream, &mut chunk).expect("the buffered bytes");
        assert!(read > 0, "the connection ended");
        waiting.extend_from_slice(&chunk[..read]);
    }
    let end = waiting.iter().position(|byte| *byte == b'\n').unwrap();
    let line = String::from_utf8(waiting[..end].to_vec()).unwrap();
    service.hand(stream, line, waiting[end + 1..].to_vec());
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(
        server.state("deck-a"),
        None,
        "a run the new image never reported"
    );
    assert_eq!(
        (
            server.evidence("deck-a").pi,
            server.evidence("deck-a").pi_signal
        ),
        (false, None),
        "the new image said no hello: it is no recognized Pi and has no coverage"
    );
    assert_eq!(probe::live(), 0, "no lease for what the old image claimed");
}
