use std::{future::Future, pin::pin, task::Poll};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

async fn pair() -> (Session, UnixStream) {
	let (client, mut peer) = UnixStream::pair().unwrap();
	let session = Session::from_stream(client, "test-user", "test-cookie")
		.await
		.unwrap();
	let mut handshake = [0; 22];
	peer.read_exact(&mut handshake).await.unwrap();

	assert_eq!(&handshake, b"test-user\ntest-cookie\n");
	(session, peer)
}

#[tokio::test]
async fn records_and_raw_response() {
	let (mut session, mut peer) = pair().await;
	peer.write_all(
        b"PAM_PROMPT_ECHO_OFF Password: \nPAM_PROMPT_ECHO_ON User: \nPAM_ERROR_MSG Error\nPAM_TEXT_INFO \nSUCCESS\n",
    )
    .await
    .unwrap();

	assert_eq!(
		session.next().await.unwrap(),
		Record::Prompt {
			text: "Password: ".into(),
			secret: true,
		}
	);

	session.respond("raw\\passwordé").await.unwrap();
	let mut response = [0; 15];
	peer.read_exact(&mut response).await.unwrap();
	assert_eq!(&response, "raw\\passwordé\n".as_bytes());

	assert_eq!(
		session.next().await.unwrap(),
		Record::Prompt {
			text: "User: ".into(),
			secret: false,
		}
	);
	assert_eq!(
		session.next().await.unwrap(),
		Record::Info {
			text: "Error".into(),
			error: true,
		}
	);
	assert_eq!(
		session.next().await.unwrap(),
		Record::Info {
			text: String::new(),
			error: false,
		}
	);
	assert_eq!(session.next().await.unwrap(), Record::Success);
}

#[tokio::test]
async fn escaped_bytes_are_decoded_before_utf8() {
	let (mut session, mut peer) = pair().await;
	peer.write_all(b"PAM_TEXT_INFO \\303\\251\\b\\f\\n\\r\\t\\v\\\\\\\"\\1A\\12B\\101\n")
		.await
		.unwrap();

	assert_eq!(
		session.next().await.unwrap(),
		Record::Info {
			text: "é\u{8}\u{c}\n\r\t\u{b}\\\"\u{1}A\nBA".into(),
			error: false,
		}
	);
}

async fn assert_pending(future: impl Future) {
	let mut future = pin!(future);
	std::future::poll_fn(|cx| {
		assert!(future.as_mut().poll(cx).is_pending());
		Poll::Ready(())
	})
	.await;
}

#[tokio::test]
async fn fragmented_input_survives_cancelled_next() {
	let (mut session, mut peer) = pair().await;
	peer.write_all(b"PAM_TEXT_INFO \\30").await.unwrap();
	// Ensure the first poll consumes the fragment, rather than just awaiting readiness.
	session
		.transport
		.as_ref()
		.unwrap()
		.get_ref()
		.readable()
		.await
		.unwrap();

	assert_pending(session.next()).await;
	assert_eq!(session.line, b"PAM_TEXT_INFO \\30");
	peer.write_all(b"3\\251\n").await.unwrap();

	assert_eq!(
		session.next().await.unwrap(),
		Record::Info {
			text: "é".into(),
			error: false,
		}
	);
}

#[tokio::test]
async fn eof_without_terminal_is_a_transport_failure() {
	for partial in [b"".as_slice(), b"PAM_TEXT_INFO truncated"] {
		let (mut session, mut peer) = pair().await;
		peer.write_all(partial).await.unwrap();
		drop(peer);

		assert_eq!(
			session.next().await.unwrap_err().kind(),
			io::ErrorKind::UnexpectedEof
		);
	}
}

#[tokio::test]
async fn malformed_records_are_rejected_without_echoing_input() {
	for malformed in [
		b"UNKNOWN sensitive\n".as_slice(),
		b"SUCCESS extra\n",
		b"PAM_TEXT_INFO\n",
		b"PAM_TEXT_INFO \\x41\n",
		b"PAM_TEXT_INFO trailing\\\n",
		b"PAM_TEXT_INFO \\000\n",
		b"PAM_TEXT_INFO \\777\n",
		b"PAM_TEXT_INFO \\377\n",
		b"PAM_TEXT_INFO raw\0nul\n",
	] {
		let (mut session, mut peer) = pair().await;
		peer.write_all(malformed).await.unwrap();

		let error = session.next().await.unwrap_err();

		assert_eq!(error.kind(), io::ErrorKind::InvalidData);
		assert_eq!(error.to_string(), "invalid helper record");
	}
}

#[tokio::test]
async fn oversized_unterminated_line_is_rejected() {
	let (mut session, mut peer) = pair().await;
	let writer = async {
		// A concurrent writer prevents socket-buffer capacity from affecting the test.
		let _ = peer.write_all(&vec![b'x'; MAX_LINE_BYTES + 1]).await;
	};
	let reader = async {
		assert_eq!(
			session.next().await.unwrap_err().kind(),
			io::ErrorKind::InvalidData
		);
	};

	tokio::join!(writer, reader);
}

#[tokio::test]
async fn exact_line_limit_is_accepted() {
	let (mut session, mut peer) = pair().await;
	let prefix = b"PAM_TEXT_INFO ";
	let text = "x".repeat(MAX_LINE_BYTES - prefix.len());
	let wire = format!("PAM_TEXT_INFO {text}\n");

	let (write, read) = tokio::join!(peer.write_all(wire.as_bytes()), session.next());

	write.unwrap();
	assert_eq!(read.unwrap(), Record::Info { text, error: false });
}

#[tokio::test]
async fn invalid_inputs_are_rejected_before_writing() {
	for invalid in [
		"bad\nvalue".into(),
		"bad\0value".into(),
		"bad\rvalue".into(),
		"x".repeat(MAX_LINE_BYTES + 1),
	] {
		for (username, cookie) in [(invalid.as_str(), "cookie"), ("user", invalid.as_str())] {
			let (client, mut peer) = UnixStream::pair().unwrap();

			let error = Session::from_stream(client, username, cookie)
				.await
				.err()
				.unwrap();

			assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
			assert_eq!(peer.read(&mut [0]).await.unwrap(), 0);
		}
	}

	let (mut session, mut peer) = pair().await;
	peer.write_all(b"PAM_PROMPT_ECHO_OFF Password\n")
		.await
		.unwrap();
	session.next().await.unwrap();

	for invalid in [
		"bad\nvalue".into(),
		"bad\0value".into(),
		"bad\rvalue".into(),
		"x".repeat(MAX_RESPONSE_BYTES + 1),
	] {
		assert_eq!(
			session.respond(&invalid).await.unwrap_err().kind(),
			io::ErrorKind::InvalidInput
		);
	}

	session.respond("").await.unwrap();
	let mut response = [0];
	peer.read_exact(&mut response).await.unwrap();
	assert_eq!(&response, b"\n");
}

#[tokio::test]
async fn terminal_records_close_without_ack() {
	for (wire, expected) in [
		(b"SUCCESS\n".as_slice(), Record::Success),
		(b"FAILURE\n", Record::Failure),
	] {
		let (mut session, mut peer) = pair().await;
		peer.write_all(wire).await.unwrap();

		assert_eq!(session.next().await.unwrap(), expected);
		assert_eq!(peer.read(&mut [0; 16]).await.unwrap(), 0);
		assert_eq!(
			session.next().await.unwrap_err().kind(),
			io::ErrorKind::NotConnected
		);
		assert_eq!(
			session.respond("answer").await.unwrap_err().kind(),
			io::ErrorKind::NotConnected
		);
	}
}

#[tokio::test]
async fn dropping_pending_conversation_closes_peer() {
	let (mut session, mut peer) = pair().await;
	let mut conversation = Box::pin(async move { session.next().await });

	assert_pending(&mut conversation).await;
	drop(conversation);

	assert_eq!(peer.read(&mut [0]).await.unwrap(), 0);
}
