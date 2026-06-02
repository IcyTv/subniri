use std::{future::Future, os::fd::OwnedFd, pin::Pin, sync::Arc, time::Duration};

use chrono::Timelike;
use config::NightlightSetting;
use rustix::fs::{MemfdFlags, memfd_create};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, Sleep};
use tokio_cron_scheduler::{Job, JobScheduler};
use wayrs_client::{
	Connection,
	global::GlobalExt,
	protocol::{
		wl_output::WlOutput,
		wl_registry::{self, GlobalArgs},
	},
};
use wayrs_protocols::wlr_gamma_control_unstable_v1::{
	ZwlrGammaControlManagerV1, ZwlrGammaControlV1, zwlr_gamma_control_v1::Event,
};
use zbus::object_server::SignalEmitter;

use crate::{NIGHTLIGHT_BUS_NAME, NIGHTLIGHT_OBJECT_PATH, NightlightPreset};

pub async fn run<F>(
	config: config::Nightlight, shutdown_signal: F,
) -> Result<(), Box<dyn std::error::Error>>
where
	F: Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
	let service = NightlightDbus::new(config.clone()).await?;
	let connection = zbus::connection::Builder::session()?
		.name(NIGHTLIGHT_BUS_NAME)?
		.serve_at(NIGHTLIGHT_OBJECT_PATH, service.clone())?
		.build()
		.await?;

	let mut sched = if config.enabled {
		activate_current_preset(service.clone(), &config).await;
		let sched = schedule_presets(service.clone(), &config).await?;
		sched.start().await?;
		Some(sched)
	} else {
		log::info!("Nightlight is disabled in config");
		None
	};
	let mut preset_reconciler = config
		.enabled
		.then(|| start_preset_reconciler(service.clone(), config.clone()));

	tokio::select! {
		result = shutdown_signal => {
			result?;
			log::info!("Shutting down");
			if let Some(reconciler) = preset_reconciler.take() {
				reconciler.abort();
			}
			service.shutdown().await;
			if let Some(sched) = &mut sched {
				sched.shutdown().await?;
			}
		}
		result = async {
			if let Some(reconciler) = &mut preset_reconciler {
				reconciler.await
			} else {
				std::future::pending().await
			}
		} => {
			result?;
			return Err("nightlight preset reconciler stopped unexpectedly".into());
		}
	}
	drop(connection);

	Ok(())
}

async fn activate_current_preset(service: NightlightDbus, config: &config::Nightlight) {
	let preset = current_preset(config);
	if let Err(error) = apply_configured_preset(&service, config, preset).await {
		log::warn!("Error activating startup {preset:?} preset: {error}");
	}
}

fn start_preset_reconciler(service: NightlightDbus, config: config::Nightlight) -> JoinHandle<()> {
	tokio::spawn(async move {
		let mut last_preset = current_preset(&config);
		let mut interval = tokio::time::interval(Duration::from_secs(60));
		interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

		loop {
			interval.tick().await;

			let preset = current_preset(&config);
			if preset == last_preset {
				continue;
			}

			log::info!("Reconciling nightlight preset after time boundary: {preset:?}");
			if let Err(error) = apply_configured_preset(&service, &config, preset).await {
				log::warn!("Error reconciling {preset:?} preset: {error}");
				continue;
			}

			last_preset = preset;
		}
	})
}

async fn apply_configured_preset(
	service: &NightlightDbus, config: &config::Nightlight, preset: NightlightPreset,
) -> zbus::fdo::Result<DesiredState> {
	let NightlightSetting {
		brightness,
		temperature,
	} = preset_setting(config, preset);

	service.apply_preset(preset, brightness, temperature).await
}

fn preset_setting(config: &config::Nightlight, preset: NightlightPreset) -> NightlightSetting {
	match preset {
		NightlightPreset::Day => config.day.clone(),
		NightlightPreset::Night => config.night.clone(),
		NightlightPreset::Custom => unreachable!("custom preset has no configured schedule"),
	}
}

fn current_preset(config: &config::Nightlight) -> NightlightPreset {
	let now = chrono::Local::now().time();
	let now = now.num_seconds_from_midnight();
	let dawn = seconds_since_midnight(config.dawn.unwrap());
	let dusk = seconds_since_midnight(config.dusk.unwrap());

	let is_day = if dawn <= dusk {
		now >= dawn && now < dusk
	} else {
		now >= dawn || now < dusk
	};

	if is_day {
		NightlightPreset::Day
	} else {
		NightlightPreset::Night
	}
}

fn seconds_since_midnight(time: jiff::civil::Time) -> u32 {
	(time.hour() as u32 * 60 * 60) + (time.minute() as u32 * 60) + time.second() as u32
}

async fn schedule_presets(
	service: NightlightDbus, config: &config::Nightlight,
) -> Result<JobScheduler, Box<dyn std::error::Error>> {
	let mut sched = JobScheduler::new().await?;

	let dawn = time_cron(config.dawn.unwrap());
	let dusk = time_cron(config.dusk.unwrap());

	sched
		.add(Job::new_async_tz(dawn, chrono::Local, {
			let service = service.clone();
			let NightlightSetting {
				brightness,
				temperature,
			} = config.day;
			move |_uuid, _l| {
				let service = service.clone();
				Box::pin(async move {
					log::info!("Dawn");
					if let Err(e) = service
						.apply_preset(NightlightPreset::Day, brightness, temperature)
						.await
					{
						log::warn!("Error activating day preset: {e}");
					};
				})
			}
		})?)
		.await?;

	sched
		.add(Job::new_async_tz(dusk, chrono::Local, {
			let service = service.clone();
			let NightlightSetting {
				brightness,
				temperature,
			} = config.night;
			move |_uuid, _l| {
				let service = service.clone();
				Box::pin(async move {
					log::info!("Dusk");
					if let Err(e) = service
						.apply_preset(NightlightPreset::Night, brightness, temperature)
						.await
					{
						log::warn!("Error activating night preset: {e}");
					};
				})
			}
		})?)
		.await?;

	let ttn = sched.time_till_next_job().await?;
	log::info!("Time till next job in scheduler: {ttn:?}");

	Ok(sched)
}

fn time_cron(time: jiff::civil::Time) -> String {
	let sec = time.second();
	let min = time.minute();
	let hour = time.hour();

	format!("{sec} {min} {hour} * * *")
}

#[derive(Clone)]
struct NightlightDbus {
	inner: Arc<NightlightDbusInner>,
}

struct NightlightDbusInner {
	available: bool,
	config: config::Nightlight,
	controller: Option<NightlightController>,
	state: Mutex<DesiredState>,
}

impl NightlightDbus {
	async fn new(config: config::Nightlight) -> Result<Self, Box<dyn std::error::Error>> {
		let controller = if config.enabled {
			Some(NightlightController::spawn(Duration::from_millis(config.debounce_ms)).await?)
		} else {
			None
		};

		Ok(Self {
			inner: Arc::new(NightlightDbusInner {
				available: config.enabled,
				config,
				controller,
				state: Mutex::new(DesiredState::default()),
			}),
		})
	}

	async fn shutdown(&self) {
		if let Some(controller) = &self.inner.controller {
			controller.shutdown().await;
		}
	}

	fn unavailable_error(&self) -> zbus::fdo::Error {
		zbus::fdo::Error::Failed("Nightlight is not enabled. Enable it in the config.".into())
	}

	async fn apply_state(&self, state: DesiredState) -> zbus::fdo::Result<DesiredState> {
		let Some(controller) = &self.inner.controller else {
			return Err(self.unavailable_error());
		};

		controller
			.set_state(state)
			.await
			.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;

		*self.inner.state.lock().await = state;
		Ok(state)
	}

	async fn apply_preset(
		&self, preset: NightlightPreset, brightness: f64, temperature: u32,
	) -> zbus::fdo::Result<DesiredState> {
		let state = DesiredState {
			active: true,
			brightness: brightness as f32,
			temperature,
			preset,
		};

		self.apply_state(state).await
	}

	async fn state(&self) -> DesiredState {
		*self.inner.state.lock().await
	}

	async fn dbus_state(&self) -> NightlightStateTuple {
		self.state().await.to_dbus(self.inner.available)
	}
}

type NightlightStateTuple = (bool, bool, f64, u32, String);

#[zbus::interface(name = "de.icytv.subniri.Nightlight")]
impl NightlightDbus {
	#[zbus(property, name = "Available")]
	fn available(&self) -> bool {
		self.inner.available
	}

	#[zbus(property, name = "Enabled")]
	async fn enabled(&self) -> bool {
		self.state().await.active
	}

	#[zbus(property, name = "Enabled")]
	async fn set_enabled(
		&self, enabled: bool, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
	) -> zbus::fdo::Result<()> {
		let mut state = self.state().await;
		state.active = enabled;
		self.apply_state(state).await?;
		self.enabled_changed(&emitter).await?;
		self.state_changed(&emitter).await?;
		Ok(())
	}

	#[zbus(property, name = "Brightness")]
	async fn brightness(&self) -> f64 {
		self.state().await.brightness as f64
	}

	#[zbus(property, name = "Brightness")]
	async fn set_brightness(
		&self, brightness: f64, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
	) -> zbus::fdo::Result<()> {
		let mut state = self.state().await;
		state.active = true;
		state.brightness = brightness as f32;
		state.preset = NightlightPreset::Custom;
		self.apply_state(state).await?;
		self.enabled_changed(&emitter).await?;
		self.brightness_changed(&emitter).await?;
		self.preset_changed(&emitter).await?;
		self.state_changed(&emitter).await?;
		Ok(())
	}

	#[zbus(property, name = "Temperature")]
	async fn temperature(&self) -> u32 {
		self.state().await.temperature
	}

	#[zbus(property, name = "Temperature")]
	async fn set_temperature(
		&self, temperature: u32, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
	) -> zbus::fdo::Result<()> {
		let mut state = self.state().await;
		state.active = true;
		state.temperature = temperature;
		state.preset = NightlightPreset::Custom;
		self.apply_state(state).await?;
		self.enabled_changed(&emitter).await?;
		self.temperature_changed(&emitter).await?;
		self.preset_changed(&emitter).await?;
		self.state_changed(&emitter).await?;
		Ok(())
	}

	#[zbus(property, name = "Preset")]
	async fn preset(&self) -> String {
		self.state().await.preset.as_str().to_string()
	}

	#[zbus(property, name = "Preset")]
	async fn set_preset(
		&self, preset: &str, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
	) -> zbus::fdo::Result<()> {
		let preset = NightlightPreset::parse(preset)?;
		let NightlightSetting {
			brightness,
			temperature,
		} = match preset {
			NightlightPreset::Day => self.inner.config.day.clone(),
			NightlightPreset::Night => self.inner.config.night.clone(),
			NightlightPreset::Custom => {
				return Err(zbus::fdo::Error::InvalidArgs(
					"custom is not a settable preset".into(),
				));
			}
		};

		self.apply_preset(preset, brightness, temperature).await?;
		self.enabled_changed(&emitter).await?;
		self.brightness_changed(&emitter).await?;
		self.temperature_changed(&emitter).await?;
		self.preset_changed(&emitter).await?;
		self.state_changed(&emitter).await?;
		Ok(())
	}

	#[zbus(property, name = "State")]
	async fn state_property(&self) -> NightlightStateTuple {
		self.dbus_state().await
	}

	async fn toggle(
		&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
	) -> zbus::fdo::Result<()> {
		let mut state = self.state().await;
		state.active = !state.active;
		self.apply_state(state).await?;
		self.enabled_changed(&emitter).await?;
		self.state_changed(&emitter).await?;
		Ok(())
	}
}

struct NightlightController {
	command_tx: mpsc::Sender<Command>,
}

impl NightlightController {
	async fn spawn(debounce: Duration) -> Result<Self, Box<dyn std::error::Error>> {
		let controller = NightlightControllerTask::new(debounce).await?;
		let (command_tx, command_rx) = mpsc::channel(8);

		tokio::spawn(async move {
			if let Err(error) = controller.run(command_rx).await {
				log::error!("nightlight controller stopped: {error}");
			}
		});

		Ok(Self { command_tx })
	}

	async fn shutdown(&self) {
		let _ = self.command_tx.send(Command::Shutdown).await;
	}

	async fn set_state(
		&self, state: DesiredState,
	) -> Result<DesiredState, Box<dyn std::error::Error>> {
		let (response_tx, response_rx) = oneshot::channel();

		self.command_tx
			.send(Command::SetState { state, response_tx })
			.await
			.map_err(|_| std::io::Error::other("nightlight controller has stopped"))?;

		Ok(response_rx
			.await
			.map_err(|_| std::io::Error::other("nightlight controller has stopped"))??)
	}
}

enum Command {
	SetState {
		state: DesiredState,
		response_tx: oneshot::Sender<std::io::Result<DesiredState>>,
	},
	Shutdown,
}

#[derive(Default)]
struct CallbackState {
	events: Vec<WaylandEvent>,
}

enum WaylandEvent {
	OutputAdded(GlobalArgs),
	OutputRemoved(u32),
	GammaSize {
		control: ZwlrGammaControlV1,
		size: u32,
	},
	GammaFailed(ZwlrGammaControlV1),
}

#[derive(Clone, Copy)]
pub struct GammaSetting {
	pub brightness: f32,
	pub temperature: u32,
}

#[derive(Clone, Copy)]
struct DesiredState {
	active: bool,
	brightness: f32,
	temperature: u32,
	preset: NightlightPreset,
}

impl Default for DesiredState {
	fn default() -> Self {
		Self {
			active: false,
			brightness: 1.0,
			temperature: 6500,
			preset: NightlightPreset::Day,
		}
	}
}

impl DesiredState {
	fn setting(self) -> GammaSetting {
		GammaSetting {
			brightness: self.brightness,
			temperature: self.temperature,
		}
	}

	fn to_dbus(self, available: bool) -> NightlightStateTuple {
		(
			self.active,
			available,
			self.brightness as f64,
			self.temperature,
			self.preset.as_str().to_string(),
		)
	}
}

struct OutputGamma {
	global_name: u32,
	output: WlOutput,
	control: ZwlrGammaControlV1,
	size: Option<u32>,
}

struct NightlightControllerTask {
	connection: Connection<CallbackState>,
	gamma_mgr: ZwlrGammaControlManagerV1,
	outputs: Vec<OutputGamma>,
	state: DesiredState,
	debounce: Duration,
	pending_apply: Option<Pin<Box<Sleep>>>,
}

impl NightlightControllerTask {
	async fn new(debounce: Duration) -> Result<Self, Box<dyn std::error::Error>> {
		let mut connection = Connection::<CallbackState>::connect()?;
		connection.async_roundtrip().await?;

		let gamma_mgr: ZwlrGammaControlManagerV1 = connection.bind_singleton(1..=1)?;
		connection.add_registry_cb(|_connection, state, event| match event {
			wl_registry::Event::Global(global) if global.is::<WlOutput>() => {
				state.events.push(WaylandEvent::OutputAdded(global.clone()));
			}
			wl_registry::Event::GlobalRemove(name) => {
				state.events.push(WaylandEvent::OutputRemoved(*name));
			}
			_ => {}
		});

		let mut controller = Self {
			connection,
			gamma_mgr,
			outputs: Vec::new(),
			state: DesiredState::default(),
			debounce,
			pending_apply: None,
		};
		let output_globals = controller
			.connection
			.globals()
			.iter()
			.filter(|g| g.is::<WlOutput>())
			.cloned()
			.collect::<Vec<_>>();

		for global in output_globals {
			controller.add_output(global)?;
		}

		controller.connection.async_roundtrip().await?;
		controller.dispatch_and_process_events().await?;

		Ok(controller)
	}

	async fn run(mut self, mut command_rx: mpsc::Receiver<Command>) -> std::io::Result<()> {
		loop {
			if let Some(pending_apply) = &mut self.pending_apply {
				tokio::select! {
					command = command_rx.recv() => {
						match command {
							Some(command) => {
								if self.handle_command(command)? {
									return Ok(());
								}
							}
							None => return Ok(()),
						}
					}
					result = self.connection.async_recv_events() => {
						result?;
						self.dispatch_and_process_events().await?;
					}
					() = pending_apply.as_mut() => {
						self.pending_apply = None;
						self.apply_state()?;
						self.connection.async_flush().await?;
					}
				}
			} else {
				tokio::select! {
					command = command_rx.recv() => {
						match command {
							Some(command) => {
								if self.handle_command(command)? {
									return Ok(());
								}
							}
							None => return Ok(()),
						}
					}
					result = self.connection.async_recv_events() => {
						result?;
						self.dispatch_and_process_events().await?;
					}
				}
			}
		}
	}

	fn schedule_apply(&mut self) {
		self.pending_apply = Some(Box::pin(tokio::time::sleep(self.debounce)));
	}

	fn handle_command(&mut self, command: Command) -> std::io::Result<bool> {
		match command {
			Command::SetState { state, response_tx } => {
				let result = self.set_state(state);
				let _ = response_tx.send(result);
				Ok(false)
			}
			Command::Shutdown => Ok(true),
		}
	}

	fn set_state(&mut self, state: DesiredState) -> std::io::Result<DesiredState> {
		self.state = state;
		self.schedule_apply();
		Ok(self.state)
	}

	fn apply_state(&mut self) -> std::io::Result<()> {
		let setting = if self.state.active {
			self.state.setting()
		} else {
			GammaSetting {
				brightness: 1.0,
				temperature: 6500,
			}
		};

		for output in &self.outputs {
			if let Some(size) = output.size {
				Self::set_gamma_for_control(
					&mut self.connection,
					output.control,
					size,
					setting.brightness,
					setting.temperature,
				)?;
			}
		}

		Ok(())
	}

	fn add_output(&mut self, global: GlobalArgs) -> std::io::Result<()> {
		if self.outputs.iter().any(|o| o.global_name == global.name) {
			return Ok(());
		}

		let output: WlOutput = global
			.bind(&mut self.connection, 1..=4)
			.map_err(|error| std::io::Error::other(format!("failed to bind output: {error}")))?;
		let control = Self::create_control(&mut self.connection, self.gamma_mgr, output);

		self.outputs.push(OutputGamma {
			global_name: global.name,
			output,
			control,
			size: None,
		});

		Ok(())
	}

	fn create_control(
		connection: &mut Connection<CallbackState>, gamma_mgr: ZwlrGammaControlManagerV1,
		output: WlOutput,
	) -> ZwlrGammaControlV1 {
		let control = gamma_mgr.get_gamma_control(connection, output);

		connection.set_callback_for(control, |ctx| match ctx.event {
			Event::GammaSize(size) => ctx.state.events.push(WaylandEvent::GammaSize {
				control: ctx.proxy,
				size,
			}),
			Event::Failed => ctx.state.events.push(WaylandEvent::GammaFailed(ctx.proxy)),
			_ => {}
		});

		control
	}

	async fn dispatch_and_process_events(&mut self) -> std::io::Result<()> {
		let mut state = CallbackState::default();
		self.connection.dispatch_events(&mut state);

		for event in state.events {
			match event {
				WaylandEvent::OutputAdded(global) => {
					log::debug!("gamma output added: {}", global.name);
					self.add_output(global)?;
					self.connection.async_flush().await?;
				}
				WaylandEvent::OutputRemoved(name) => {
					log::debug!("gamma output removed: {name}");
					self.remove_output(name);
					self.connection.async_flush().await?;
				}
				WaylandEvent::GammaSize { control, size } => {
					if let Some(output) = self.outputs.iter_mut().find(|o| o.control == control) {
						log::debug!(
							"got gamma ramp size {} for output {}",
							size,
							output.global_name
						);
						output.size = Some(size);
					}

					self.schedule_apply();
				}
				WaylandEvent::GammaFailed(control) => {
					log::warn!("gamma control failed: {control:?}");
					self.recreate_control(control);
					self.connection.async_flush().await?;
				}
			}
		}

		Ok(())
	}

	fn remove_output(&mut self, global_name: u32) {
		if let Some(index) = self
			.outputs
			.iter()
			.position(|o| o.global_name == global_name)
		{
			let output = self.outputs.remove(index);
			output.control.destroy(&mut self.connection);
			output.output.release(&mut self.connection);
		}
	}

	fn recreate_control(&mut self, control: ZwlrGammaControlV1) {
		if let Some(index) = self.outputs.iter().position(|o| o.control == control) {
			let output = &mut self.outputs[index];
			output.control.destroy(&mut self.connection);
			output.control =
				Self::create_control(&mut self.connection, self.gamma_mgr, output.output);
			output.size = None;
		}
	}

	fn set_gamma_for_control(
		connection: &mut Connection<CallbackState>, controls: ZwlrGammaControlV1, size: u32,
		brightness: f32, temperature: u32,
	) -> std::io::Result<()> {
		if size < 2 {
			return Err(std::io::Error::other("invalid gamma ramp size"));
		}

		let (r, g, b) = temperature_to_rgb_normalized(temperature);

		let fd = memfd_create("gamma-lut", MemfdFlags::empty())?;

		let file = std::fs::File::from(fd);

		file.set_len(size as u64 * 3 * 2)?;

		let mut mmap = unsafe { memmap2::MmapMut::map_mut(&file)? };

		let mut idx = 0;

		let mut push_col = |channel: f32| {
			for i in 0..size {
				let x = i as f32 / (size - 1) as f32;
				let wchar = (x * channel * brightness * u16::MAX as f32).clamp(0.0, 65535.0) as u16;
				let bytes = wchar.to_ne_bytes();
				mmap[idx..idx + 2].copy_from_slice(&bytes);

				idx += 2;
			}
		};

		push_col(r);
		push_col(g);
		push_col(b);

		controls.set_gamma(connection, OwnedFd::from(file));

		Ok(())
	}
}

impl Drop for NightlightControllerTask {
	fn drop(&mut self) {
		for output in self.outputs.drain(..) {
			output.control.destroy(&mut self.connection);
			output.output.release(&mut self.connection);
		}

		self.gamma_mgr.destroy(&mut self.connection);

		let _ = self.connection.flush(wayrs_client::IoMode::Blocking);
	}
}

fn temperature_to_rgb_normalized(temperature: u32) -> (f32, f32, f32) {
	let rgb = raw_temperature_to_rgb(temperature);
	let white = raw_temperature_to_rgb(6500);

	(
		(rgb.0 / white.0).clamp(0.0, 1.0),
		(rgb.1 / white.1).clamp(0.0, 1.0),
		(rgb.2 / white.2).clamp(0.0, 1.0),
	)
}

fn raw_temperature_to_rgb(temperature: u32) -> (f32, f32, f32) {
	debug_assert!(temperature >= 1000 && temperature <= 40_000);

	let t = temperature as f32 / 100.0;

	let r = if t <= 66.0 {
		255.0
	} else {
		329.698727446 * (t - 60.0).powf(-0.1332047592)
	};

	let g = if t <= 66.0 {
		99.4708025861 * t.ln() - 161.1195681661
	} else {
		288.1221695283 * (t - 60.0).powf(-0.0755148492)
	};

	let b = if t >= 66.0 {
		255.0
	} else if t <= 19.0 {
		0.0
	} else {
		138.5177312231 * (t - 10.0).ln() - 305.0447927307
	};

	let r = r.clamp(0.0, 255.0) / 255.0;
	let g = g.clamp(0.0, 255.0) / 255.0;
	let b = b.clamp(0.0, 255.0) / 255.0;

	(r, g, b)
}
