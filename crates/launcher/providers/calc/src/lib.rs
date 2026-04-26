use std::sync::Arc;

use async_channel::{Receiver, Sender};
use kalkulator::Expression;
use launcher_common::*;

const CALC_PROVIDER_ID: ProviderId = ProviderId("calc");

pub struct CalcProvider {
	sender: Sender<ProviderEvent>,
	receiver: Receiver<ProviderEvent>,
}

impl CalcProvider {
	pub fn new() -> Self {
		let (sender, receiver) = async_channel::unbounded();
		Self { sender, receiver }
	}
}

impl CalcProvider {
	fn candidate_from_res(expr_str: Arc<str>, res: f64) -> Candidate {
		let res_str = Arc::<str>::from(format!("{res:.8}").trim_end_matches("0").trim_end_matches("."));
		Candidate {
			provider: CALC_PROVIDER_ID,
			id: Self::cand_id(),
			activation: ActivationKey(res_str.clone()),
			title: res_str.clone(),
			right_text: Some(expr_str),
			subtitle: None,
			icon: None,
			kind: CandidateKind::Calc,
			section_hint: Some(SectionHint::Calculations),
			match_kind: MatchKind::Exact,
			provider_score: 10.,
		}
	}

	fn cand_id() -> CandidateId {
		CandidateId(Arc::from("calc0"))
	}
}

#[async_trait::async_trait]
impl Provider for CalcProvider {
	fn id(&self) -> ProviderId {
		CALC_PROVIDER_ID
	}
	fn name(&self) -> &'static str {
		"Calculator"
	}

	async fn init(
		&self, _ctx: Arc<dyn ProviderContext>, _rt: RuntimeHandle,
	) -> anyhow::Result<async_channel::Receiver<ProviderEvent>> {
		Ok(self.receiver.clone())
	}

	async fn update_query(
		&self, _session: SessionHandle, query: Query, _ctx: Arc<dyn ProviderContext>, rt: RuntimeHandle,
	) -> anyhow::Result<()> {
		let expr_str = query.raw;
		self.sender.send(ProviderEvent::Status(ProviderStatus::Loading)).await?;
		if !looks_like_expression(&expr_str) {
			self.sender
				.send(ProviderEvent::CandidateRemove { id: Self::cand_id() })
				.await?;
			self.sender.send(ProviderEvent::Done).await?;

			return Ok(());
		}
		let e = expr_str.clone();
		let res = rt
			.spawn_blocking(move || -> Result<f64, &'static str> {
				let mut expr = Expression::new(&e);
				expr.infix_to_postfix().map_err(|e| e.as_str())?;
				expr.compute_expression().map_err(|e| e.as_str())?;
				expr.get_result().as_ref().map_err(|e| e.as_str()).copied()
			})
			.await?;

		let res = match res {
			Ok(res) => res,
			Err(e) => {
				self.sender
					.send(ProviderEvent::Status(ProviderStatus::Error(Arc::from(e))))
					.await?;
				// TODO: What do we ACTUALLY return here?
				return Ok(());
			}
		};

		self.sender
			.send(ProviderEvent::CandidateUpsert(Self::candidate_from_res(expr_str, res)))
			.await?;
		self.sender.send(ProviderEvent::Done).await?;

		Ok(())
	}

	// async fn begin(
	// 	&self, _session: SessionHandle, query: Query, _ctx: Arc<dyn ProviderContext>, rt: RuntimeHandle,
	// ) -> anyhow::Result<BoxStream<'static, ProviderEvent>> {
	// 	Ok(Self::stream(query.raw, rt).boxed())
	// }
	//
	// async fn update(
	// 	&self, _session: SessionHandle, query: Query, _ctx: Arc<dyn ProviderContext>, rt: RuntimeHandle,
	// ) -> anyhow::Result<BoxStream<'static, ProviderEvent>> {
	// 	Ok(Self::stream(query.raw, rt).boxed())
	// }

	async fn activate(
		&self, _session: SessionHandle, _candidate_id: &CandidateId, activation: &ActivationKey, _rt: RuntimeHandle,
	) -> anyhow::Result<Activation> {
		let mut clipboard = arboard::Clipboard::new()?;
		clipboard.set_text(&*activation.0)?;
		Ok(Activation::SetResponse(activation.0.to_string()))
	}
}

fn looks_like_expression(s: &str) -> bool {
	let s = s.trim();
	if s.is_empty() || s.len() > 128 {
		return false;
	}
	let mut has_digit = false;
	let mut depth = 0i32;
	let mut prev_was_op = true; // allow unary at start
	for ch in s.chars() {
		if ch.is_ascii_digit() {
			has_digit = true;
			prev_was_op = false;
			continue;
		}
		if ch.is_whitespace() || ch == '.' || ch == ',' {
			continue;
		}
		match ch {
			'(' => {
				depth += 1;
				prev_was_op = true;
			}
			')' => {
				if depth == 0 || prev_was_op {
					return false;
				}
				depth -= 1;
				prev_was_op = false;
			}
			'+' | '-' | '*' | '/' | '%' | '^' | '!' => {
				if prev_was_op && ch != '+' && ch != '-' {
					return false;
				}
				prev_was_op = true;
			}
			_ => return false,
		}
	}
	has_digit && depth == 0
}
