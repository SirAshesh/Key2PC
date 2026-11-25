use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use key2pc_core::{
    start_configured_service, write_error_log, AppConfig, JsonFileConfigStore, ManagedService,
    Role, ServiceStatus, StatusObserver, UpdateConfig,
};

pub struct AppHost {
    store: JsonFileConfigStore,
    service: Option<Box<dyn ManagedService>>,
    show_latency: bool,
    log_file: Option<String>,
    update_config: UpdateConfig,
    observer: StatusObserver,
}

impl AppHost {
    pub fn new(
        store: JsonFileConfigStore,
        latest_status: Arc<Mutex<ServiceStatus>>,
        status_dirty: Arc<AtomicBool>,
        wake_fn: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let observer: StatusObserver = Arc::new(move |snapshot: &ServiceStatus| {
            if let Ok(mut guard) = latest_status.lock() {
                *guard = snapshot.clone();
            }
            status_dirty.store(true, Ordering::Release);
            wake_fn();
        });

        Self {
            store,
            service: None,
            show_latency: true,
            log_file: None,
            update_config: UpdateConfig::default(),
            observer,
        }
    }

    pub fn start_or_reload(&mut self) {
        if let Some(mut old) = self.service.take() {
            old.shutdown();
        }

        match self.store.load_or_create_default() {
            Ok(config) => self.boot_with_config(config),
            Err(err) => {
                let msg = err.to_string();
                write_error_log(self.log_file.as_deref(), &msg);
                let err_status = ServiceStatus::error(Role::Sender, "Key2PC".to_string(), msg);
                (self.observer)(&err_status);
            }
        }
    }

    pub fn show_latency(&self) -> bool {
        self.show_latency
    }

    pub fn log_file(&self) -> Option<&str> {
        self.log_file.as_deref()
    }

    pub fn update_config(&self) -> UpdateConfig {
        self.update_config.clone()
    }

    pub fn toggle_active(&self) {
        if let Some(svc) = &self.service {
            svc.toggle_active();
        }
    }

    pub fn toggle_display(&self) {
        if let Some(svc) = &self.service {
            svc.toggle_display();
        }
    }

    pub fn toggle_fullscreen(&self) {
        if let Some(svc) = &self.service {
            svc.toggle_fullscreen();
        }
    }

    pub fn toggle_virtual_monitor(&self) {
        if let Some(svc) = &self.service {
            svc.toggle_virtual_monitor();
        }
    }

    pub fn install_virtual_driver(&self) {
        if let Some(svc) = &self.service {
            if let Err(err) = svc.install_virtual_driver() {
                write_error_log(self.log_file.as_deref(), &err.to_string());
            }
        }
    }

    pub fn release_all(&self) {
        if let Some(svc) = &self.service {
            svc.release_all();
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(mut svc) = self.service.take() {
            svc.shutdown();
        }
    }

    fn boot_with_config(&mut self, config: AppConfig) {
        self.show_latency = config.tray.show_tooltip_latency;
        self.log_file = config.log_file.clone();
        self.update_config = config.update.clone();

        match start_configured_service(&config, Arc::clone(&self.observer)) {
            Ok(service) => {
                let status = service.status();
                (self.observer)(&status);
                self.service = Some(service);
            }
            Err(err) => {
                let msg = err.to_string();
                write_error_log(self.log_file.as_deref(), &msg);
                let err_status = ServiceStatus::error(config.role, config.device_name.clone(), msg);
                (self.observer)(&err_status);
            }
        }
    }
}
