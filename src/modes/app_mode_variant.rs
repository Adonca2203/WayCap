use crate::application_config::AppModeDbus;

use super::{shadow_cap::ShadowCapMode, AppMode};

pub enum AppModeVariant {
    Shadow(ShadowCapMode),
}

impl AppMode for AppModeVariant {
    async fn init(&mut self, ctx: &mut crate::app_context::AppContext) -> anyhow::Result<()> {
        match self {
            AppModeVariant::Shadow(mode) => mode.init(ctx).await,
        }
    }

    async fn on_save(&mut self, ctx: &mut crate::app_context::AppContext) -> anyhow::Result<()> {
        match self {
            AppModeVariant::Shadow(mode) => mode.on_save(ctx).await,
        }
    }

    async fn on_exit(&mut self, ctx: &mut crate::app_context::AppContext) -> anyhow::Result<()> {
        match self {
            AppModeVariant::Shadow(mode) => mode.on_exit(ctx).await,
        }
    }

    async fn on_shutdown(
        &mut self,
        ctx: &mut crate::app_context::AppContext,
    ) -> anyhow::Result<()> {
        match self {
            AppModeVariant::Shadow(mode) => mode.on_shutdown(ctx).await,
        }
    }
}

impl std::fmt::Debug for AppModeVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppModeVariant::Shadow(_) => write!(f, "Shadow Capture Mode"),
        }
    }
}

impl AppModeVariant {
    pub fn to_dbus(&self) -> AppModeDbus {
        match self {
            AppModeVariant::Shadow(_) => AppModeDbus::Shadow,
        }
    }
}
