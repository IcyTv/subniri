use async_channel::{Receiver, Sender};

use crate::providers::ProviderEvent;

pub struct ProjectProvider {
	sender: Sender<ProviderEvent>,
	receiver: Receiver<ProviderEvent>,
}
