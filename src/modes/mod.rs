pub mod app_mode_variant;
pub mod shadow_cap;
use crate::app_context::AppContext;
use anyhow::Result;

pub trait AppMode: Send + 'static {
    async fn init(&mut self, ctx: &mut AppContext) -> Result<()>;
    async fn on_save(&mut self, ctx: &mut AppContext) -> Result<()>;
    async fn on_shutdown(&mut self, ctx: &mut AppContext) -> Result<()>;
    async fn on_exit(&mut self, ctx: &mut AppContext) -> Result<()>;
}
