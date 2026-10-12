use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A masked client frame header; the payload is not needed to screen.
fn header(last: bool, opcode: u8, length: u64) -> Vec<u8> {
    let mut header = vec![if last { 0x80 } else { 0 } | opcode];
    match length {
        0..=125 => header.push(0x80 | length as u8),
        126..=65535 => {
            header.push(0x80 | 126);
            header.extend_from_slice(&(length as u16).to_be_bytes());
        }
        _ => {
            header.push(0x80 | 127);
            header.extend_from_slice(&length.to_be_bytes());
        }
    }
    header.extend_from_slice(&[1, 2, 3, 4]);
    header
}

fn frame(last: bool, opcode: u8, length: usize) -> Vec<u8> {
    let mut frame = header(last, opcode, length as u64);
    frame.resize(frame.len() + length, 0xaa);
    frame
}

#[test]
fn invalid_control_reserved_opcodes_and_rsv_bits_are_rejected_at_the_header() {
    for (last, opcode, length) in [
        (true, 9, 126),
        (false, 9, 1),
        (true, 3, 1),
        (true, 11, 1),
        (true, 0x41, 1),
    ] {
        for chunk in [1, MAX_HEADER] {
            let mut screen = FrameScreen::default();
            let bytes = header(last, opcode, length);
            let result = bytes
                .chunks(chunk)
                .try_for_each(|bytes| screen.feed(bytes, super::super::MAX_MESSAGE as u64));
            assert_eq!(result, Err(FrameError::Invalid));
            assert_eq!(screen.payload_left, 0, "no payload was admitted");
        }
    }
}

#[tokio::test]
async fn revocation_prevents_buffered_websocket_data_from_being_flushed() {
    use futures::SinkExt;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::{Message, protocol::Role};
    let (mut peer, ours) = tokio::io::duplex(1);
    let meter = Meter::new();
    let closed = tokio_util::sync::CancellationToken::new();
    meter.bind_session(closed.clone());
    meter.authenticate();
    let stream = Metered::new(ours, meter, super::super::PRE_AUTH_MAX_MESSAGE);
    let mut socket = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
    {
        let sending = socket.send(Message::text("private".repeat(10000)));
        tokio::pin!(sending);
        assert!(futures::poll!(&mut sending).is_pending());
        closed.cancel();
        assert!(
            sending.await.is_err(),
            "revocation interrupts the pending transport write"
        );
    }
    let mut first = [0; 1];
    peer.read_exact(&mut first).await.unwrap();
    assert_eq!(first[0], 0x81, "only the earlier frame header was written");
    assert!(
        socket.flush().await.is_err(),
        "closing cannot flush the queued private payload"
    );
    drop(socket);
    let mut remaining = Vec::new();
    peer.read_to_end(&mut remaining).await.unwrap();
    assert!(
        remaining.is_empty(),
        "no bytes were written after revocation"
    );
}

#[tokio::test]
async fn oversized_control_headers_never_reach_the_library_read_boundary() {
    for authenticated in [false, true] {
        let (mut peer, ours) = tokio::io::duplex(MAX_HEADER);
        let meter = Meter::new();
        if authenticated {
            meter.authenticate();
        }
        let mut stream = Metered::new(ours, meter.clone(), super::super::PRE_AUTH_MAX_MESSAGE);
        peer.write_all(&header(true, 9, super::super::MAX_MESSAGE as u64))
            .await
            .unwrap();
        let mut bytes = [0; MAX_HEADER];
        let error = stream
            .read(&mut bytes)
            .await
            .expect_err("the allocating parser receives an error, never the header");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(meter.invalid());
        assert_eq!(stream.screen.payload_left, 0);
    }
}

#[test]
fn a_frame_header_is_parsed_at_every_length_width() {
    for length in [0u64, 125, 126, 65535, 65536, 1 << 40] {
        let bytes = header(true, 0x1, length);
        assert_eq!(Frame::parse(&bytes[..bytes.len() - 1]), None, "{length}");
        assert_eq!(
            Frame::parse(&bytes),
            Some(Frame {
                reserved: false,
                last: true,
                opcode: 0x1,
                length
            })
        );
    }
    assert_eq!(Frame::parse(&[0x01]), None);
}

#[test]
fn messages_are_screened_whole_however_the_bytes_arrive() {
    let mut stream = frame(true, 0x1, 100);
    stream.extend(frame(true, 0x9, 5));
    stream.extend(frame(true, 0x1, 100));
    for chunk in [1, 3, 7, stream.len()] {
        let mut screen = FrameScreen::default();
        for piece in stream.chunks(chunk) {
            assert_eq!(screen.feed(piece, 100), Ok(()), "chunks of {chunk}");
        }
        assert_eq!(screen.message, 0, "each message ended");
    }
    let mut screen = FrameScreen::default();
    assert_eq!(
        screen.feed(&header(true, 0x1, 101), 100),
        Err(FrameError::TooLong)
    );
    let mut huge = FrameScreen::default();
    assert_eq!(
        huge.feed(&header(true, 0x2, u64::MAX), 100),
        Err(FrameError::TooLong)
    );
}

#[test]
fn fragments_add_up_and_control_frames_do_not() {
    let mut screen = FrameScreen::default();
    assert_eq!(screen.feed(&frame(false, 0x1, 60), 100), Ok(()));
    assert_eq!(screen.feed(&frame(true, 0x9, 125), 100), Ok(()));
    assert_eq!(screen.feed(&frame(false, 0x0, 40), 100), Ok(()));
    assert_eq!(
        screen.feed(&header(true, 0x0, 1), 100),
        Err(FrameError::TooLong)
    );
}

#[tokio::test]
async fn an_unauthenticated_peer_is_cut_off_at_the_oversized_header() {
    let (mut peer, ours) = tokio::io::duplex(1024);
    let meter = Meter::new();
    let mut metered = Metered::new(ours, meter.clone(), 8);
    let small = frame(true, 0x1, 8);
    peer.write_all(&small).await.expect("write");
    let mut read = vec![0u8; small.len()];
    metered
        .read_exact(&mut read)
        .await
        .expect("within the limit");
    peer.write_all(&header(true, 0x1, 9)).await.expect("write");
    let refused = metered.read(&mut read).await.expect_err("over the limit");
    assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
    assert!(meter.overran());
}

#[tokio::test]
async fn authentication_widens_the_data_budget_and_writes_stamp_progress() {
    let (mut peer, ours) = tokio::io::duplex(1024);
    let meter = Meter::new();
    let mut metered = Metered::new(ours, meter.clone(), 4);
    meter.authenticate();
    let large = frame(true, 0x1, 64);
    peer.write_all(&large).await.expect("write");
    let mut all = vec![0u8; large.len()];
    metered.read_exact(&mut all).await.expect("no limit now");
    assert!(!meter.overran());

    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(meter.since_write_progress() >= Duration::from_millis(30));
    metered.write_all(b"x").await.expect("write");
    assert!(meter.since_write_progress() < Duration::from_millis(30));
}

#[tokio::test]
async fn rejected_headers_cannot_resume_allocating_reads_during_close() {
    for opcode in [1, 9] {
        let (mut peer, ours) = tokio::io::duplex(MAX_HEADER);
        let meter = Meter::new();
        let mut stream = Metered::new(ours, meter, super::super::PRE_AUTH_MAX_MESSAGE);
        peer.write_all(&header(true, opcode, super::super::MAX_MESSAGE as u64))
            .await
            .unwrap();
        let mut bytes = [0; MAX_HEADER];
        assert_eq!(
            stream.read(&mut bytes).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let fake_payload_header = header(true, 1, super::super::MAX_MESSAGE as u64);
        peer.write_all(&fake_payload_header).await.unwrap();
        assert_eq!(
            stream.read(&mut bytes).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            stream.raw().read(&mut bytes).await.unwrap(),
            fake_payload_header.len(),
            "closing can only discard raw bytes"
        );
    }
}
