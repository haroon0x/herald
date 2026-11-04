//! Build the provider adapters this process can use.
//!
//! A provider that has no adapter still parses and plans; the registry simply
//! does not contain it, so the CLI can defer honestly instead of pretending a
//! read happened.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use herald_config::{Config, SmtpSecurity as ConfigSmtpSecurity};
use herald_core::ProviderId;
use herald_provider::fake::FakeProvider;
use herald_provider::maildir::{MaildirConfig, MaildirProvider, SmtpConfig, SmtpSecurity};
use herald_provider::x::{XConfig, XProvider};
use herald_provider::{CommunicationProvider, ProviderError};

use crate::error::AppError;

pub struct Registry {
    adapters: BTreeMap<ProviderId, Arc<dyn CommunicationProvider>>,
    warnings: Vec<String>,
}

impl Registry {
    #[must_use]
    pub fn build(config: &Config, data_dir: &Path) -> Self {
        let mut adapters: BTreeMap<ProviderId, Arc<dyn CommunicationProvider>> = BTreeMap::new();
        let mut warnings = Vec::new();

        let demo = FakeProvider::with_state(data_dir.join("demo-provider.json"));
        // A deterministic failure hook for the demo provider. It is only read
        // from the environment, so normal use never sees it.
        match std::env::var("HERALD_DEMO_FAIL_NEXT_SEND").as_deref() {
            Ok("before") => demo.fail_next_send(ProviderError::Timeout),
            Ok("after") => demo.fail_next_send_after_delivery(ProviderError::Timeout),
            _ => {}
        }
        adapters.insert(ProviderId::Fake, Arc::new(demo));

        if let Some(maildir) = &config.maildir {
            match build_maildir(config, maildir) {
                Ok((provider, warning)) => {
                    if let Some(warning) = warning {
                        warnings.push(warning);
                    }
                    adapters.insert(ProviderId::Maildir, Arc::new(provider));
                }
                Err(error) => warnings.push(format!("maildir provider disabled: {error}")),
            }
        }

        if let Some(x) = &config.x {
            let profile_dir = config.x_profile_dir(data_dir);
            if herald_provider::x::detect_chrome().is_none() {
                warnings.push(
                    "the X adapter needs Chrome or Chromium; install it or set CHROME before using x"
                        .into(),
                );
            } else if !profile_dir.exists() {
                warnings.push(format!(
                    "the X profile {} is not initialised; run `herald x login`",
                    profile_dir.display()
                ));
            }
            adapters.insert(
                ProviderId::X,
                Arc::new(XProvider::new(
                    XConfig {
                        profile_dir,
                        handle: x.handle.clone(),
                        headless: x.headless,
                        action_delay_ms: x.action_delay_ms,
                        typing_delay_ms: x.typing_delay_ms,
                        close_after: x.close_after,
                        browser_executable: None,
                    },
                    data_dir.to_path_buf(),
                )),
            );
        }

        Self { adapters, warnings }
    }

    #[must_use]
    pub fn get(&self, provider: ProviderId) -> Option<Arc<dyn CommunicationProvider>> {
        self.adapters.get(&provider).cloned()
    }

    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

fn build_maildir(
    config: &Config,
    maildir: &herald_config::MaildirSettings,
) -> Result<(MaildirProvider, Option<String>), AppError> {
    let Some((root, sent_path)) = config.maildir_paths() else {
        return Err(AppError::new("config", "maildir section is incomplete"));
    };

    let mut warning = None;
    let smtp = if let Some(smtp) = &maildir.smtp {
        if let Ok(password) = smtp.resolve_password(|key| std::env::var(key).ok()) {
            Some(SmtpConfig {
                host: smtp.host.clone(),
                port: smtp.port,
                username: smtp.username.clone(),
                password,
                security: match smtp.security {
                    ConfigSmtpSecurity::Starttls => SmtpSecurity::Starttls,
                    ConfigSmtpSecurity::Tls => SmtpSecurity::Tls,
                    ConfigSmtpSecurity::None => SmtpSecurity::None,
                },
            })
        } else {
            warning = Some(format!(
                "SMTP password is unavailable; the maildir is read-only. Set {} or unset maildir.smtp.username.",
                smtp.password_env
                    .as_deref()
                    .unwrap_or("HERALD_SMTP_PASSWORD")
            ));
            None
        }
    } else {
        None
    };

    let provider = MaildirProvider::new(MaildirConfig {
        root,
        sent_path: Some(sent_path),
        from: maildir.from.clone(),
        smtp,
    });

    Ok((provider, warning))
}
