use std::{io, net::SocketAddr, time::Duration};

use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::{TcpListener, TcpStream},
};

pub const OAUTH_CALLBACK_TIMEOUT: Duration = Duration::from_mins(5);

const MAX_REQUEST_SIZE: usize = 16 * 1024;

pub struct Callback {
	pub code: String,
	pub state: String,
	stream: TcpStream,
}

impl Callback {
	pub async fn success(mut self) {
		if let Err(error) = send_page(
			&mut self.stream,
			"200 OK",
			"Authentication complete",
			"You can close this window and return to snowconf.",
		)
		.await
		{
			log::warn!("Failed to send OAuth success page: {error}");
		}
	}

	pub async fn failure(mut self) {
		if let Err(error) = send_page(
			&mut self.stream,
			"400 Bad Request",
			"Authentication failed",
			"Return to snowconf to try again.",
		)
		.await
		{
			log::warn!("Failed to send OAuth failure page: {error}");
		}
	}
}

pub struct CallbackListener {
	listener: TcpListener,
	redirect_uri: url::Url,
}

impl CallbackListener {
	pub async fn bind(
		redirect_uri: &str,
	) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
		let redirect_uri = url::Url::parse(redirect_uri)?;
		if redirect_uri.scheme() != "http" {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"OAuth callback URL must use HTTP",
			)
			.into());
		}
		let host = redirect_uri.host_str().ok_or_else(|| {
			io::Error::new(
				io::ErrorKind::InvalidInput,
				"OAuth callback URL has no host",
			)
		})?;
		let address: SocketAddr = format!(
			"{host}:{}",
			redirect_uri
				.port_or_known_default()
				.ok_or_else(|| io::Error::new(
					io::ErrorKind::InvalidInput,
					"OAuth callback URL has no port"
				),)?
		)
		.parse()?;
		if !address.ip().is_loopback() {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"OAuth callback listener must bind to a loopback address",
			)
			.into());
		}

		Ok(Self {
			listener: TcpListener::bind(address).await?,
			redirect_uri,
		})
	}

	pub async fn receive(self) -> Result<Callback, Box<dyn std::error::Error + Send + Sync>> {
		let (mut stream, _) = self.listener.accept().await?;
		match self.read_callback(&mut stream).await {
			Ok((code, state)) => Ok(Callback {
				code,
				state,
				stream,
			}),
			Err(error) => {
				if let Err(response_error) = send_page(
					&mut stream,
					"400 Bad Request",
					"Authentication failed",
					"Return to snowconf to try again.",
				)
				.await
				{
					log::warn!("Failed to send OAuth failure page: {response_error}");
				}
				Err(error)
			}
		}
	}

	async fn read_callback(
		&self, stream: &mut TcpStream,
	) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
		let mut request = Vec::with_capacity(1024);
		loop {
			if request.len() >= MAX_REQUEST_SIZE {
				return Err(io::Error::new(
					io::ErrorKind::InvalidData,
					"OAuth callback request was too large",
				)
				.into());
			}
			let mut chunk = [0; 1024];
			let read = stream.read(&mut chunk).await?;
			if read == 0 {
				break;
			}
			let bytes = chunk.get(..read).ok_or_else(|| {
				io::Error::new(io::ErrorKind::InvalidData, "Invalid OAuth callback request")
			})?;
			request.extend_from_slice(bytes);
			if request.windows(4).any(|window| window == b"\r\n\r\n") {
				break;
			}
		}

		let request = std::str::from_utf8(&request)?;
		let target = request
			.lines()
			.next()
			.and_then(|line| line.split_whitespace().nth(1))
			.ok_or_else(|| {
				io::Error::new(
					io::ErrorKind::InvalidData,
					"Invalid OAuth callback request line",
				)
			})?;
		let callback = self.redirect_uri.join(target)?;
		if callback.path() != self.redirect_uri.path() {
			return Err(io::Error::new(
				io::ErrorKind::InvalidData,
				"OAuth callback path did not match the redirect URL",
			)
			.into());
		}

		let mut code = None;
		let mut state = None;
		let mut provider_error = None;
		for (key, value) in callback.query_pairs() {
			match key.as_ref() {
				"code" => code = Some(value.into_owned()),
				"state" => state = Some(value.into_owned()),
				"error_description" | "error" => provider_error = Some(value.into_owned()),
				_ => {}
			}
		}
		if let Some(error) = provider_error {
			return Err(io::Error::other(format!("OAuth provider rejected login: {error}")).into());
		}

		Ok((
			code.ok_or_else(|| {
				io::Error::new(
					io::ErrorKind::InvalidData,
					"OAuth callback contained no code",
				)
			})?,
			state.ok_or_else(|| {
				io::Error::new(
					io::ErrorKind::InvalidData,
					"OAuth callback contained no state",
				)
			})?,
		))
	}
}

async fn send_page(
	stream: &mut TcpStream, status: &str, heading: &str, message: &str,
) -> io::Result<()> {
	let body = format!(
		"<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{heading}</title><style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#151515;color:#f4f4f0;font:18px system-ui,sans-serif}}main{{max-width:32rem;padding:3rem;border:2px solid #f4f4f0;border-radius:8px;box-shadow:8px 8px 0 #55d187}}h1{{margin-top:0;font-size:2rem}}p{{margin-bottom:0;color:#c8c8c2}}</style></head><body><main><h1>{heading}</h1><p>{message}</p></main></body></html>"
	);
	let response = format!(
		"HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
		body.len()
	);
	stream.write_all(response.as_bytes()).await
}
