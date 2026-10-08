use crate::overlay::OverlayState;
use crate::{
    navigation::Page,
    views::market_table::{CategoryFilter, PriceTable},
};
use gpui_kit::component::{
    Root,
    input::{InputEvent, InputState},
    table::{ColumnSort, TableEvent, TableState},
};
use gpui_kit::*;
use poe2_core::{GameId, Quote};
use poe2_service::{
    AppState, Command, ServiceHandle,
    update_activity::{UpdateActivity, UpdatePhase, UpdateTarget},
};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone)]
struct MainWorkspace {
    view: WeakEntity<Toolkit>,
    window: AnyWindowHandle,
}
impl Global for MainWorkspace {}

struct MarketPosition {
    scope: Option<poe2_core::MarketScope>,
    selected: Option<Quote>,
    offset: Point<Pixels>,
}

/// Menu and tray exits follow the same draft and transaction protections.
pub(crate) fn request_quit(services: &ServiceHandle, cx: &mut App) {
    if services.snapshot().busy {
        return;
    }
    let main = cx.try_global::<MainWorkspace>().cloned();
    if let Some(main) = main.filter(|main| cx.windows().contains(&main.window)) {
        let _ = main
            .window
            .update(cx, |_, window, cx| {
                main.view
                    .update(cx, |this, cx| {
                        if this.busy() {
                            return;
                        }
                        this.persist_layout(window);
                        if this.dirty() {
                            window.activate_window();
                            cx.activate(true);
                            this.confirm_close(true, window, cx);
                        } else {
                            cx.quit();
                        }
                    })
                    .is_ok()
            })
            .unwrap_or(false);
        // An update failure must not turn into an unconfirmed data-losing exit.
        return;
    }
    cx.quit();
}

/// Route restart through the main workspace so a settings-window action cannot
/// discard an unsaved patch draft in another window. Window access is deferred.
pub(crate) fn request_application_install(requester: WeakEntity<Toolkit>, cx: &mut App) {
    if let Some(main) = cx
        .try_global::<MainWorkspace>()
        .cloned()
        .filter(|main| cx.windows().contains(&main.window))
    {
        let _ = main.window.update(cx, |_, window, cx| {
            let _ = main.view.update(cx, |this, cx| {
                if !this.busy() && this.application_presentation().can_install {
                    this.navigate(crate::overlay::Intent::InstallApplication, window, cx);
                }
            });
        });
    } else {
        let _ = requester.update(cx, |this, cx| {
            if !this.busy() && this.application_presentation().can_install {
                this.dispatch(Command::InstallApplication, cx);
            }
        });
    }
}

pub struct Toolkit {
    pub(crate) services: ServiceHandle,
    pub(crate) state: Arc<AppState>,
    pub(crate) page: Page,
    pub(crate) market_picker_open: bool,
    pub(crate) market_draft: std::rc::Rc<std::cell::RefCell<poe2_core::Realm>>,
    pub(crate) overlay: OverlayState,
    pub(crate) draft: Option<crate::draft::DraftSession>,
    pub(crate) changes: Entity<TableState<crate::views::change_table::ChangeTable>>,
    pub(crate) review_plans: BTreeMap<GameId, Arc<poe2_core::PatchPlan>>,
    pub(crate) patch_changes: bool,
    pub(crate) selected_change: Option<usize>,
    pub(crate) nav_expanded: bool,
    pub(crate) inspector_width: f32,
    pub(crate) resizing_inspector: bool,
    pub(crate) cancellable: bool,
    pub(crate) quality_only: bool,
    quality_filters: BTreeMap<GameId, bool>,
    patch_tabs: BTreeMap<GameId, bool>,
    market_positions: BTreeMap<GameId, MarketPosition>,
    pages: BTreeMap<GameId, Page>,
    pub(crate) drawer_focus: FocusHandle,
    pub(crate) drawer_was_open: bool,
    pub(crate) drawer_closing: bool,
    pub(crate) last_scene: Option<crate::motion::Scene>,
    pub(crate) scene_generation: usize,
    pub(crate) nav_hover: Option<Page>,
    pub(crate) patch_hover: Option<bool>,
    pub(crate) drawer_return_focus: Option<FocusHandle>,
    pub(crate) saving_draft: bool,
    searches: BTreeMap<GameId, Entity<InputState>>,
    categories: BTreeMap<GameId, CategoryFilter>,
    sorts: BTreeMap<GameId, Option<(usize, ColumnSort)>>,
    pub(crate) search: Entity<InputState>,
    pub(crate) table: Entity<TableState<PriceTable>>,
    pub(crate) category: CategoryFilter,
    pub(crate) selected: Option<Quote>,
    pub(crate) pending: bool,
    pub(crate) local_error: Option<String>,
    pub(crate) local_update: Option<(UpdateTarget, UpdateActivity)>,
    pub(crate) focus: FocusHandle,
    pub(crate) settings_only: bool,
    pub(crate) native_backdrop: bool,
    pub(crate) show_patch_warnings: bool,
    pub(crate) show_market_warnings: bool,
    settings_window: Option<WindowHandle<Root>>,
    _subscriptions: Vec<Subscription>,
    _updates: Task<()>,
}

impl Toolkit {
    pub fn new(
        services: ServiceHandle,
        settings_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let native_backdrop = crate::window_chrome::initialize(window, cx);
        let state = services.snapshot();
        let layout = crate::layout::LayoutPreferences::load(&state.data_dir);
        let focus = cx.focus_handle();
        if !settings_only {
            cx.set_global(MainWorkspace {
                view: cx.entity().downgrade(),
                window: window.window_handle(),
            });
        }
        focus.focus(window, cx);
        let searches: BTreeMap<_, _> = GameId::ALL
            .into_iter()
            .map(|game| {
                (
                    game,
                    cx.new(|cx| InputState::new(window, cx).placeholder("搜索物品名称或资源身份…")),
                )
            })
            .collect();
        let search = searches[&state.profile.game].clone();
        let rows = state
            .snapshot
            .as_ref()
            .map(|s| (0..s.quotes.len()).collect())
            .unwrap_or_default();
        let table = cx.new(|cx| {
            TableState::new(
                PriceTable {
                    snapshot: state.snapshot.clone(),
                    rows,
                    sort: None,
                    selected_id: None,
                    restoring_selection: false,
                },
                window,
                cx,
            )
            // Quotes are selected as records, not spreadsheet cells.
            .row_selectable(true)
            .cell_selectable(false)
            .col_selectable(false)
        });
        let changes = cx.new(|cx| {
            TableState::new(
                crate::views::change_table::ChangeTable { plan: None },
                window,
                cx,
            )
            .row_selectable(true)
            .cell_selectable(false)
            .col_selectable(false)
        });
        let mut subscriptions: Vec<_> = searches
            .values()
            .map(|search| {
                cx.subscribe(search, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.filter_table(cx);
                        cx.notify();
                    }
                })
            })
            .collect();
        subscriptions.push(cx.subscribe(
            &table,
            |this, table, event: &TableEvent, cx| match event {
                TableEvent::SelectRow(row) => {
                    if this.dirty() {
                        return;
                    }
                    let delegate = table.read(cx).delegate();
                    this.selected = delegate
                        .snapshot
                        .as_ref()
                        .and_then(|s| delegate.rows.get(*row).map(|n| s.quotes[*n].clone()));
                    let restoring = delegate.restoring_selection;
                    let id = this.selected.as_ref().map(|q| q.item_id.clone());
                    table.update(cx, |table, _| {
                        table.delegate_mut().selected_id = id;
                        table.delegate_mut().restoring_selection = false;
                    });
                    if !restoring {
                        this.show_drawer(OverlayState::Quote);
                    }
                    cx.notify();
                }
                TableEvent::ClearSelection => {
                    this.selected = None;
                    table.update(cx, |table, _| table.delegate_mut().selected_id = None);
                    if matches!(this.overlay, OverlayState::Quote) {
                        this.overlay = OverlayState::None;
                    }
                    cx.notify();
                }
                _ => {}
            },
        ));
        subscriptions.push(cx.subscribe(&changes, |this, _, event: &TableEvent, cx| {
            if let TableEvent::SelectRow(row) = event {
                this.selected_change = Some(*row);
                this.show_drawer(OverlayState::Change);
                cx.notify();
            }
        }));
        subscriptions.push(cx.observe_window_appearance(window, |this, window, cx| {
            crate::theme::sync(Some(window), cx);
            this.native_backdrop = crate::window_chrome::initialize(window, cx);
            cx.notify();
        }));
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            crate::theme::sync(Some(window), cx);
            this.native_backdrop = crate::window_chrome::initialize(window, cx);
            crate::motion::sync_system(cx);
            cx.notify();
        }));
        subscriptions.push(cx.observe_global::<crate::motion::MotionSettings>(|_, cx| cx.notify()));
        subscriptions.push(cx.observe_global_in::<crate::theme::AppearanceSettings>(
            window,
            |this, window, cx| {
                this.native_backdrop = crate::window_chrome::initialize(window, cx);
                cx.notify();
            },
        ));
        let mut receiver = services.state.clone();
        let updates = cx.spawn(async move |weak, cx| {
            while receiver.changed().await.is_ok() {
                let next = receiver.borrow_and_update().clone();
                if weak
                    .update(cx, |this, cx| {
                        if this.local_update.as_ref().is_some_and(|(target, local)| {
                            next.update_activity(*target).generation >= local.generation
                        }) {
                            this.local_update = None;
                        }
                        let game_changed = this.state.profile.game != next.profile.game;
                        if game_changed {
                            this.market_positions.insert(
                                this.state.profile.game,
                                MarketPosition {
                                    scope: this.state.profile.market.clone(),
                                    selected: this.selected.clone(),
                                    offset: this
                                        .table
                                        .read(cx)
                                        .vertical_scroll_handle
                                        .0
                                        .borrow()
                                        .base_handle
                                        .offset(),
                                },
                            );
                            this.pages.insert(this.state.profile.game, this.page);
                            this.quality_filters
                                .insert(this.state.profile.game, this.quality_only);
                            this.patch_tabs
                                .insert(this.state.profile.game, this.patch_changes);
                            this.page = this
                                .pages
                                .get(&next.profile.game)
                                .copied()
                                .unwrap_or(Page::Patch);
                            this.patch_changes = this
                                .patch_tabs
                                .get(&next.profile.game)
                                .copied()
                                .unwrap_or(false);
                            this.quality_only = this
                                .quality_filters
                                .get(&next.profile.game)
                                .copied()
                                .unwrap_or(false);
                            this.market_picker_open = false;
                            this.sorts.insert(
                                this.state.profile.game,
                                this.table.read(cx).delegate().sort,
                            );
                            let sort = this.sorts.get(&next.profile.game).copied().flatten();
                            this.table
                                .update(cx, |table, _| table.delegate_mut().sort = sort);
                            this.categories
                                .insert(this.state.profile.game, this.category);
                            this.category = this
                                .categories
                                .get(&next.profile.game)
                                .copied()
                                .unwrap_or(CategoryFilter::All);
                            this.search = this.searches[&next.profile.game].clone();
                            if !this.dirty() || this.drawer_closing {
                                this.overlay = OverlayState::None;
                                this.drawer_closing = false;
                            }
                        }
                        if this.saving_draft && !next.busy {
                            let saved = this
                                .draft
                                .as_ref()
                                .is_some_and(|draft| draft.acknowledged(&next.profile));
                            if saved && next.error.is_none() {
                                this.draft = None;
                            }
                            if saved || next.error.is_some() {
                                this.saving_draft = false;
                            }
                        }
                        let scope_changed = this.state.profile.market != next.profile.market;
                        let plan_changed = this.state.plan.as_ref().map(|p| &p.id)
                            != next.plan.as_ref().map(|p| &p.id);
                        if plan_changed || game_changed {
                            if let Some(plan) = &next.plan {
                                this.review_plans.insert(next.profile.game, plan.clone());
                                if !game_changed && !this.dirty() {
                                    this.page = Page::Patch;
                                    this.patch_changes = true;
                                    this.close_drawer();
                                }
                            }
                            this.show_patch_warnings = false;
                            let plan = this.review_plans.get(&next.profile.game).cloned();
                            if game_changed
                                || this
                                    .changes
                                    .read(cx)
                                    .delegate()
                                    .plan
                                    .as_ref()
                                    .map(|p| &p.id)
                                    != plan.as_ref().map(|p| &p.id)
                            {
                                this.selected_change = None;
                                if matches!(this.overlay, OverlayState::Change) {
                                    this.close_drawer();
                                }
                                this.changes.update(cx, |table, cx| {
                                    table.delegate_mut().plan = plan;
                                    table.clear_selection(cx);
                                    table.refresh(cx);
                                });
                            }
                        }
                        let changed = game_changed
                            || scope_changed
                            || this.state.snapshot.as_ref().map(|s| &s.id)
                                != next.snapshot.as_ref().map(|s| &s.id);
                        if scope_changed && !game_changed {
                            this.category = CategoryFilter::All;
                        }
                        this.state = next;
                        if game_changed
                            && !this
                                .state
                                .leagues
                                .iter()
                                .any(|l| l.game == this.state.profile.game)
                        {
                            let _ = this.services.send(Command::DiscoverLeagues);
                        }
                        this.pending = false;
                        this.table.update(cx, |_, cx| cx.notify());
                        if changed {
                            this.show_market_warnings = false;
                            if game_changed || scope_changed {
                                this.selected = None;
                            }
                            if game_changed {
                                let position = this
                                    .market_positions
                                    .get(&this.state.profile.game)
                                    .filter(|p| p.scope == this.state.profile.market);
                                this.selected = position.and_then(|p| p.selected.clone());
                                let offset = position.map(|p| p.offset).unwrap_or_default();
                                this.table.update(cx, |table, _| {
                                    table
                                        .vertical_scroll_handle
                                        .0
                                        .borrow()
                                        .base_handle
                                        .set_offset(offset);
                                });
                            }
                            this.filter_table(cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let closing_service = services.clone();
        let owner = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if closing_service.snapshot().busy {
                return false;
            }
            owner
                .update(cx, |this, cx| {
                    this.persist_layout(window);
                    if this.dirty() {
                        this.confirm_close(false, window, cx);
                        false
                    } else {
                        true
                    }
                })
                .unwrap_or(true)
        });
        let mut result = Self {
            market_picker_open: false,
            market_draft: Default::default(),
            overlay: OverlayState::None,
            draft: None,
            changes,
            review_plans: BTreeMap::new(),
            patch_changes: false,
            selected_change: None,
            nav_expanded: layout.navigation_expanded,
            inspector_width: layout.inspector,
            resizing_inspector: false,
            cancellable: false,
            quality_only: false,
            quality_filters: BTreeMap::new(),
            patch_tabs: BTreeMap::new(),
            market_positions: BTreeMap::new(),
            pages: BTreeMap::new(),
            drawer_focus: cx.focus_handle(),
            drawer_was_open: false,
            drawer_closing: false,
            last_scene: None,
            scene_generation: 0,
            nav_hover: None,
            patch_hover: None,
            drawer_return_focus: None,
            saving_draft: false,
            searches,
            categories: BTreeMap::new(),
            sorts: BTreeMap::new(),
            services,
            state,
            page: if settings_only {
                Page::Settings
            } else {
                Page::Patch
            },
            settings_only,
            native_backdrop,
            show_patch_warnings: false,
            show_market_warnings: false,
            settings_window: None,
            focus,
            search,
            table,
            category: CategoryFilter::All,
            selected: None,
            pending: false,
            local_error: None,
            local_update: None,
            _subscriptions: subscriptions,
            _updates: updates,
        };
        if !settings_only
            && !result.state.busy
            && !result
                .state
                .leagues
                .iter()
                .any(|league| league.game == result.state.profile.game)
        {
            result.dispatch(Command::DiscoverLeagues, cx);
        }
        result
    }
    pub(crate) fn busy(&self) -> bool {
        self.pending
            || self.state.busy
            || self
                .local_update
                .as_ref()
                .is_some_and(|(target, activity)| {
                    *target == UpdateTarget::Rules && activity.phase.running()
                })
    }
    pub(crate) fn dispatch(&mut self, command: Command, cx: &mut Context<Self>) {
        let update_target = command.update_target();
        if let Some(target) = update_target {
            if self.busy() {
                return;
            }
            let mut activity = self.state.update_activity(target).clone();
            activity.start(if matches!(command, Command::InstallApplication) {
                UpdatePhase::Installing
            } else {
                UpdatePhase::Checking
            });
            self.local_update = Some((target, activity));
        }
        self.cancellable = matches!(command, Command::Apply(_));
        self.local_error = None;
        match self.services.send(command) {
            Ok(()) => self.pending = true,
            Err(e) => {
                if update_target.is_some() {
                    if let Some((_, activity)) = self.local_update.as_mut() {
                        activity.finish(Err(e.to_string()));
                    }
                } else {
                    self.local_error = Some(e.to_string());
                }
            }
        };
        cx.notify();
    }
    pub(crate) fn filter_table(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).text().to_string().to_lowercase();
        let snapshot = self.state.snapshot.clone();
        let category = self.category;
        let selected_id = self.selected.as_ref().map(|q| q.item_id.clone());
        let quality_only = self.quality_only;
        self.selected = snapshot.as_ref().and_then(|s| {
            s.quotes
                .iter()
                .find(|q| Some(&q.item_id) == selected_id.as_ref())
                .cloned()
        });
        self.table.update(cx, |table, cx| {
            let d = table.delegate_mut();
            d.rows = snapshot
                .as_ref()
                .map(|s| {
                    s.quotes
                        .iter()
                        .enumerate()
                        .filter(|(_, q)| {
                            category.matches(q)
                                && (!quality_only
                                    || q.quality_at(chrono::Utc::now())
                                        != poe2_core::Quality::Fresh)
                                && (q.name.to_lowercase().contains(&query)
                                    || q.item_id.to_lowercase().contains(&query))
                        })
                        .map(|(i, _)| i)
                        .collect()
                })
                .unwrap_or_default();
            d.snapshot = snapshot;
            d.sort_rows();
            let selected_row = d.snapshot.as_ref().and_then(|s| {
                d.rows
                    .iter()
                    .position(|i| Some(&s.quotes[*i].item_id) == selected_id.as_ref())
            });
            d.selected_id = selected_id.clone();
            table.refresh(cx);
            if let Some(row) = selected_row {
                table.delegate_mut().restoring_selection = true;
                table.set_selected_row(row, cx);
                // A quote refresh must not pull the viewport back to an old
                // selection the user has already scrolled away from.
                table
                    .vertical_scroll_handle
                    .0
                    .borrow_mut()
                    .deferred_scroll_to_item = None;
            } else {
                table.clear_selection(cx);
            }
        });
    }
    pub(crate) fn select_client_path(&mut self, files: bool, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files,
            directories: !files,
            multiple: false,
            prompt: Some(
                if files {
                    "选择游戏程序、Content.ggpk 或 _.index.bin"
                } else {
                    "选择游戏安装目录"
                }
                .into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    this.dispatch(Command::ConnectDetected(path), cx)
                });
            }
        })
        .detach();
    }
    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        if !crate::theme::MAC {
            self.page = Page::Settings;
            cx.notify();
            return;
        }
        if let Some(handle) = self.settings_window
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        let service = self.services.clone();
        let bounds = Bounds::centered(None, size(px(760.), px(730.)), cx);
        match cx.open_window(
            crate::window_chrome::options("设置", bounds, size(px(680.), px(540.))),
            move |window, cx| {
                let view = cx.new(|cx| Toolkit::new(service, true, window, cx));
                cx.new(|cx| Root::new(view, window, cx).bg(transparent_black()))
            },
        ) {
            Ok(handle) => self.settings_window = Some(handle),
            Err(error) => self.local_error = Some(format!("无法打开设置：{error}")),
        }
    }
}
