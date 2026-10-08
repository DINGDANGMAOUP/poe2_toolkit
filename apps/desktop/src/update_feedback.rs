//! Update availability and transfer feedback; routine checks stay in Settings.
use crate::{app::Toolkit, components::muted, theme::Palette};
use gpui_kit::{
    component::{Icon, IconName},
    prelude::FluentBuilder,
    *,
};
use poe2_service::update_activity::{UpdateActivity, UpdatePhase, UpdateTarget};

impl Toolkit {
    pub(crate) fn shows_update_error(&self, error: &str, target: UpdateTarget) -> bool {
        matches!(&self.update_activity(target).phase, UpdatePhase::Failed(message) if message == error)
    }

    pub(crate) fn update_activity(&self, target: UpdateTarget) -> &UpdateActivity {
        self.local_update
            .as_ref()
            .filter(|(local_target, _)| *local_target == target)
            .map(|(_, activity)| activity)
            .unwrap_or_else(|| self.state.update_activity(target))
    }

    pub(crate) fn application_presentation(&self) -> ApplicationPresentation {
        ApplicationPresentation::new(
            self.update_activity(UpdateTarget::Application),
            &self.state.application_update,
            self.state.application_download_started,
        )
    }

    pub(crate) fn rule_update_feedback(&self, cx: &App) -> AnyElement {
        let p = Palette::of(cx);
        let activity = self.update_activity(UpdateTarget::Rules);
        let failed = matches!(activity.phase, UpdatePhase::Failed(_));
        let text = match &activity.phase {
            UpdatePhase::Idle if self.state.update_configured => {
                "检查后自动验证并更新规则。".to_owned()
            }
            UpdatePhase::Idle => "使用内置规则，尚未配置发行通道。".into(),
            UpdatePhase::Succeeded(message) => format!("{message}{}", checked_at(activity)),
            UpdatePhase::Failed(message) => format!("检查失败：{message}"),
            _ => "正在检查并验证规则…".into(),
        };
        div()
            .id("rules-feedback")
            .role(Role::Status)
            .aria_label(text.clone())
            .px_4()
            .pb_3()
            .flex()
            .gap_2()
            .items_start()
            .when(failed, |el| {
                el.child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(13.))
                        .text_color(p.danger),
                )
            })
            .child(muted(text, p))
            .into_any_element()
    }
}

/// Presentation precedence matters: a failed retry may still have a verified
/// package on disk; a running retry must not offer a competing restart action.
#[derive(Clone, Debug)]
pub(crate) struct ApplicationPresentation {
    pub phase: UpdatePhase,
    pub title: String,
    pub detail: String,
    pub caption: String,
    pub running: bool,
    pub failed: bool,
    pub ready: bool,
    pub available: bool,
    pub toolbar_visible: bool,
    pub can_install: bool,
    pub needs_installer: bool,
    pub configured: bool,
    pub progress: Option<f32>,
}

impl ApplicationPresentation {
    fn new(
        activity: &UpdateActivity,
        status: &poe2_service::app_updates::Status,
        download_started: bool,
    ) -> Self {
        let pending = status.pending_version.as_deref();
        let installed = status.installed;
        let configured = status.configured;
        let available = status.available_version.is_some();
        let running = activity.phase.running();
        let ready = pending.is_some() && !running;
        let needs_installer = !installed && (ready || available) && !running;
        let (title, detail, caption) = match &activity.phase {
            UpdatePhase::Checking => (
                "正在检查更新…".into(),
                "正在连接发行源。".into(),
                "正在检查新版本…".into(),
            ),
            UpdatePhase::Downloading { received, total } => {
                let bytes = format!(
                    "{:.1} / {:.1} MB",
                    *received as f64 / 1_048_576.,
                    *total as f64 / 1_048_576.
                );
                (
                    "正在下载更新…".into(),
                    format!("{bytes} · 完成后验证安装包。"),
                    format!("正在下载 · {bytes}"),
                )
            }
            UpdatePhase::Verifying => (
                "正在验证更新…".into(),
                "正在检查签名和安装包完整性。".into(),
                "正在验证安装包…".into(),
            ),
            UpdatePhase::Installing => (
                "正在准备重启…".into(),
                "正在备份配置，随后重启并完成安装。".into(),
                "正在准备重启安装…".into(),
            ),
            _ if needs_installer => (
                "需要完整发行包".into(),
                "当前目录缺少更新组件。请安装发行版或完整解压便携包，再使用重启更新。".into(),
                "当前目录不支持重启安装，请获取完整发行包。".into(),
            ),
            UpdatePhase::Failed(message) => (
                "更新未完成".into(),
                message.clone(),
                format!("更新未完成：{message}"),
            ),
            _ if ready => {
                let version = pending.unwrap_or_default();
                (
                    format!("版本 {version} 已就绪"),
                    "更新已下载并通过验证，重启后即可使用。".into(),
                    format!("版本 {version} 已下载，可重启安装。"),
                )
            }
            _ if available => {
                let version = status.available_version.as_deref().unwrap_or_default();
                (
                    format!("发现新版本 {version}"),
                    "点击下载更新；完成后由你选择何时重启。".into(),
                    format!("有新版本 {version}，可下载更新。"),
                )
            }
            UpdatePhase::Succeeded(_) => (
                "已是最新版本".into(),
                format!(
                    "当前版本 {}{}",
                    env!("CARGO_PKG_VERSION"),
                    checked_at(activity)
                ),
                format!("已是最新版本{}", checked_at(activity)),
            ),
            _ if !configured => (
                "尚未配置更新通道".into(),
                "当前没有受信任的程序发行通道。".into(),
                "尚未配置程序发行通道。".into(),
            ),
            _ => (
                "软件更新".into(),
                "检查后自动下载更新；准备就绪后，由你选择何时重启。".into(),
                "内测版".into(),
            ),
        };
        Self {
            phase: activity.phase.clone(),
            title,
            detail,
            caption,
            running,
            failed: matches!(activity.phase, UpdatePhase::Failed(_)),
            ready,
            available,
            toolbar_visible: ready
                || available
                || matches!(
                    activity.phase,
                    UpdatePhase::Downloading { .. }
                        | UpdatePhase::Verifying
                        | UpdatePhase::Installing
                )
                || (download_started && matches!(activity.phase, UpdatePhase::Failed(_))),
            can_install: ready && installed,
            needs_installer,
            configured,
            progress: match activity.phase {
                UpdatePhase::Downloading { received, total } if total > 0 => {
                    Some((received as f32 / total as f32 * 100.).clamp(0., 100.))
                }
                _ => None,
            },
        }
    }

    pub fn toolbar_label(&self) -> &'static str {
        if matches!(self.phase, UpdatePhase::Installing) {
            "正在重启…"
        } else if self.running {
            if matches!(self.phase, UpdatePhase::Checking) {
                "准备下载…"
            } else {
                button_label(&self.phase)
            }
        } else if self.can_install {
            "重启更新"
        } else if self.needs_installer {
            "获取安装包"
        } else if self.failed {
            "重试下载"
        } else {
            "有新版本"
        }
    }

    pub fn settings_label(&self) -> &'static str {
        if self.running {
            button_label(&self.phase)
        } else if self.can_install {
            "重启更新"
        } else if self.needs_installer {
            "获取安装包"
        } else if self.failed {
            "重试"
        } else if self.available {
            "下载更新"
        } else {
            "检查更新"
        }
    }

    pub fn motion_step(&self) -> usize {
        match self.phase {
            UpdatePhase::Checking => 1,
            UpdatePhase::Downloading { .. } => 2,
            UpdatePhase::Verifying => 3,
            UpdatePhase::Installing => 4,
            UpdatePhase::Failed(_) => 5,
            _ if self.ready => 6,
            _ if self.available => 7,
            _ => 0,
        }
    }
}

fn checked_at(activity: &UpdateActivity) -> String {
    activity
        .completed_at
        .map(|time| {
            format!(
                " · {} 检查",
                time.with_timezone(&chrono::Local).format("%H:%M")
            )
        })
        .unwrap_or_default()
}

pub(crate) fn button_label(phase: &UpdatePhase) -> &'static str {
    match phase {
        UpdatePhase::Checking => "正在检查…",
        UpdatePhase::Downloading { .. } => "正在下载…",
        UpdatePhase::Verifying => "正在校验…",
        UpdatePhase::Installing => "正在准备重启…",
        UpdatePhase::Failed(_) => "重试",
        _ => "检查更新",
    }
}
