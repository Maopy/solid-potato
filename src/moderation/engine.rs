//! Deterministic planning only. This core has no network or page executor.
use super::{events::*, templates::Template};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

const SILENCE: u64 = 5_000;
const WELCOME_WINDOW: u64 = 30_000;
const GROUP_RETENTION: u64 = 600_000;
const LEVEL_RETENTION: u64 = 1_800_000;

#[derive(Clone, Debug)]
pub struct RulesConfig {
    pub gift_enabled: bool,
    pub gift_show_quantity: bool,
    pub welcome_enabled: bool,
    pub supporter_welcome_enabled: bool,
    pub fansclub_joined_enabled: bool,
    pub fansclub_upgrade_enabled: bool,
    pub honor_upgrade_enabled: bool,
    pub wealth_upgrade_enabled: bool,
    pub gift_template: String,
    pub gift_emojis: Vec<String>,
    pub gift_variation: super::gift_text::GiftVariation,
    pub gift_nicknames: super::nicknames::Nicknames,
    pub overflow_template: String,
    pub welcome_template: String,
    pub supporter_welcome_template: String,
    pub fansclub_joined_template: String,
    pub upgrade_template: String,
    pub allow_inferred_levels: bool,
    pub max_text_chars: usize,
    pub max_active_users: usize,
    pub max_gift_items: usize,
    pub max_pending_actions: usize,
    pub max_dedupe_entries: usize,
}

impl Default for RulesConfig {
    fn default() -> Self {
        Self {
            gift_enabled: true,
            gift_show_quantity: true,
            welcome_enabled: true,
            supporter_welcome_enabled: true,
            fansclub_joined_enabled: true,
            fansclub_upgrade_enabled: true,
            honor_upgrade_enabled: true,
            wealth_upgrade_enabled: true,
            gift_template: "感谢{nickname}送来的{gift_list}".into(),
            // Offline/legacy planning remains deterministic. YAML defaults enable suffixes.
            gift_emojis: vec![],
            gift_variation: Default::default(),
            gift_nicknames: Default::default(),
            overflow_template: "感谢{nickname}，感谢礼物飘屏".into(),
            welcome_template: "欢迎{nickname}来到直播间".into(),
            supporter_welcome_template: "欢迎{title}{nickname}来到直播间，感谢支持！".into(),
            fansclub_joined_template: "欢迎{nickname}加入粉丝团".into(),
            upgrade_template: "恭喜{nickname}的{level_name}升级到{new_level}级".into(),
            allow_inferred_levels: false,
            max_text_chars: 120,
            max_active_users: 500,
            max_gift_items: 100,
            max_pending_actions: 20,
            max_dedupe_entries: 10_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    GiftThanks,
    Welcome,
    Upgrade,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GiftItem {
    #[serde(default)]
    pub unit_value: Option<u64>,
    pub gift_id: String,
    pub name: String,
    pub quantity: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GiftBatch {
    pub batch_id: String,
    pub revision: u64,
    pub user_id: String,
    pub nickname: String,
    pub recipient_user_id: String,
    pub last_gift_at: u64,
    pub due_at: u64,
    pub items: Vec<GiftItem>,
    pub event_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreviewAction {
    pub action_id: String,
    pub mode: String,
    pub scope: Scope,
    pub user_id: String,
    pub kind: ActionKind,
    pub priority: u8,
    pub text: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub evidence: Evidence,
    pub gift_batch: Option<GiftBatch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Audit {
    pub at_ms: u64,
    pub scope: Scope,
    pub kind: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<PreviewAction>,
}

fn log(
    out: &mut Vec<Audit>,
    at: u64,
    scope: &Scope,
    kind: &str,
    reason: &str,
    event: Option<&str>,
    action: Option<PreviewAction>,
) {
    out.push(Audit {
        at_ms: at,
        scope: scope.clone(),
        kind: kind.into(),
        reason: reason.into(),
        event_id: event.map(str::to_owned),
        action,
    });
}

struct Window {
    batch: GiftBatch,
    planned: Option<String>,
}
struct Group {
    mode: QuantityMode,
    count: u64,
    seen_at: u64,
}
struct Baseline {
    value: u32,
    source: String,
    at: u64,
}
struct WelcomeWindow {
    due_at: u64,
    users: Vec<(String, String)>,
}

#[derive(Default)]
struct Room {
    paused: bool,
    dedupe: HashMap<String, (u64, String)>,
    native_ids: HashMap<(String, String, String), u64>,
    groups: HashMap<(String, String, Option<String>), Group>,
    batches: HashMap<String, Window>,
    pending: BTreeMap<String, PreviewAction>,
    welcomes: HashMap<String, u64>,
    welcome_window: Option<WelcomeWindow>,
    welcome_actions: HashMap<String, Vec<(String, String)>>,
    levels: HashMap<(String, String), Baseline>,
    congratulated: HashMap<(String, String, u32), u64>,
}

pub struct Engine {
    config: RulesConfig,
    gift_text: super::gift_text::GiftText,
    welcome: Template,
    supporter_welcome: Template,
    fansclub_joined: Template,
    upgrade: Template,
    rooms: BTreeMap<Scope, Room>,
    clock: u64,
    action_seq: u64,
    batch_seq: u64,
}

impl Engine {
    pub fn new(config: RulesConfig) -> Result<Self> {
        if !(1..=500).contains(&config.max_active_users)
            || !(1..=100).contains(&config.max_gift_items)
            || !(1..=20).contains(&config.max_pending_actions)
            || !(1..=20_000).contains(&config.max_dedupe_entries)
            || !(1..=500).contains(&config.max_text_chars)
        {
            bail!("invalid offline core resource limits");
        }
        Ok(Self {
            gift_text: super::gift_text::GiftText::new(
                &config.gift_template,
                &config.overflow_template,
                &config.gift_emojis,
                config.max_text_chars,
            )?
            .with_variation(&config.gift_variation)?
            .with_nicknames(&config.gift_nicknames)?,
            welcome: Template::parse(&config.welcome_template, &["nickname"])?,
            supporter_welcome: Template::parse(
                &config.supporter_welcome_template,
                &["nickname", "title"],
            )?,
            fansclub_joined: Template::parse(&config.fansclub_joined_template, &["nickname"])?,
            upgrade: Template::parse(
                &config.upgrade_template,
                &["nickname", "level_name", "new_level"],
            )?,
            config,
            rooms: BTreeMap::new(),
            clock: 0,
            action_seq: 0,
            batch_seq: 0,
        })
    }

    fn check_clock(&self, now: u64) -> Result<()> {
        if now < self.clock || now > u64::MAX - LEVEL_RETENTION {
            bail!("virtual clock must be monotonic and allow deadline arithmetic");
        }
        Ok(())
    }

    /// Ingest all events at a timestamp before advance(timestamp). Exact-deadline
    /// new gifts win over old timers, without retaining stale timer entries.
    pub fn ingest(&mut self, now: u64, event: CoreEvent) -> Result<Vec<Audit>> {
        self.check_clock(now)?;
        event.validate()?;
        let mut out = self.advance_inner(now, false)?;
        if matches!(event.kind, EventKind::Unsupported) {
            log(
                &mut out,
                now,
                &event.scope,
                "suppressed",
                "unsupported_event",
                Some(&event.event_id),
                None,
            );
            return Ok(out);
        }
        if !self.rooms.contains_key(&event.scope) && self.rooms.len() >= 16 {
            log(
                &mut out,
                now,
                &event.scope,
                "suppressed",
                "scope_limit",
                Some(&event.event_id),
                None,
            );
            return Ok(out);
        }
        let mut room = self.rooms.remove(&event.scope).unwrap_or_default();
        let result = self.process(now, &event, &mut room, &mut out);
        self.rooms.insert(event.scope.clone(), room);
        result?;
        Ok(out)
    }

    pub fn advance(&mut self, now: u64) -> Result<Vec<Audit>> {
        self.advance_inner(now, true)
    }

    fn advance_inner(&mut self, now: u64, inclusive: bool) -> Result<Vec<Audit>> {
        self.check_clock(now)?;
        self.clock = now;
        let mut out = Vec::new();
        let rooms = std::mem::take(&mut self.rooms);
        for (scope, mut room) in rooms {
            room.welcome_actions
                .retain(|id, _| room.pending.contains_key(id));
            room.dedupe
                .retain(|_, (at, _)| now.saturating_sub(*at) <= GROUP_RETENTION);
            room.native_ids
                .retain(|_, at| now.saturating_sub(*at) <= GROUP_RETENTION);
            room.groups
                .retain(|_, group| now.saturating_sub(group.seen_at) <= GROUP_RETENTION);
            room.welcomes
                .retain(|_, at| now.saturating_sub(*at) <= 60_000);
            room.levels
                .retain(|_, baseline| now.saturating_sub(baseline.at) <= LEVEL_RETENTION);
            room.congratulated
                .retain(|_, at| now.saturating_sub(*at) <= LEVEL_RETENTION);
            let mut due: Vec<_> = room
                .batches
                .iter()
                .filter(|(_, w)| {
                    w.planned.is_none()
                        && (w.batch.due_at < now || inclusive && w.batch.due_at == now)
                })
                .map(|(user, w)| (w.batch.due_at, user.clone()))
                .collect();
            due.sort();
            for (at, user) in due {
                self.expire(at, true, &scope, &mut room, &mut out);
                if let Some(window) = room.batches.get(&user) {
                    let batch = window.batch.clone();
                    let text = self.gift_text(&batch)?;
                    let action = self.action(
                        at,
                        &scope,
                        &user,
                        ActionKind::GiftThanks,
                        text,
                        Evidence::Verified,
                        Some(batch),
                    );
                    if self.plan(action.clone(), &mut room, &mut out) {
                        room.batches.get_mut(&user).expect("existing batch").planned =
                            Some(action.action_id);
                    } else {
                        room.batches.remove(&user);
                    }
                }
            }
            if room
                .welcome_window
                .as_ref()
                .is_some_and(|w| w.due_at < now || inclusive && w.due_at == now)
            {
                let window = room.welcome_window.take().expect("due welcome window");
                self.expire(window.due_at, true, &scope, &mut room, &mut out);
                if !window.users.is_empty() {
                    let action = self.action(
                        window.due_at,
                        &scope,
                        &window.users[0].0,
                        ActionKind::Welcome,
                        self.welcome_text(&window.users)?,
                        Evidence::Verified,
                        None,
                    );
                    if self.plan(action.clone(), &mut room, &mut out) {
                        room.welcome_actions.insert(action.action_id, window.users);
                    }
                }
            }
            self.expire(now, inclusive, &scope, &mut room, &mut out);
            self.rooms.insert(scope, room);
        }
        Ok(out)
    }

    fn expire(
        &self,
        now: u64,
        inclusive: bool,
        scope: &Scope,
        room: &mut Room,
        out: &mut Vec<Audit>,
    ) {
        let mut ids: Vec<_> = room
            .pending
            .values()
            .filter(|a| a.expires_at < now || inclusive && a.expires_at == now)
            .map(|a| (a.expires_at, a.action_id.clone()))
            .collect();
        ids.sort();
        for (_, id) in ids {
            let action = room.pending.remove(&id).expect("pending");
            if room
                .batches
                .get(&action.user_id)
                .is_some_and(|w| w.planned.as_deref() == Some(id.as_str()))
            {
                room.batches.remove(&action.user_id);
            }
            log(
                out,
                action.expires_at,
                scope,
                "expired",
                "preview_ttl",
                None,
                Some(action),
            );
        }
    }

    fn pause(&self, now: u64, scope: &Scope, room: &mut Room, reason: &str, out: &mut Vec<Audit>) {
        room.paused = true;
        for (_, action) in std::mem::take(&mut room.pending) {
            log(out, now, scope, "cancelled", reason, None, Some(action));
        }
        room.batches.clear();
        room.welcome_window = None;
        room.welcome_actions.clear();
        room.groups.clear();
        room.levels.clear();
        room.congratulated.clear();
        log(out, now, scope, "paused", reason, None, None);
    }

    fn process(
        &mut self,
        now: u64,
        e: &CoreEvent,
        room: &mut Room,
        out: &mut Vec<Audit>,
    ) -> Result<()> {
        let suppress = |out: &mut Vec<Audit>, reason: &str| {
            log(
                out,
                now,
                &e.scope,
                "suppressed",
                reason,
                Some(&e.event_id),
                None,
            )
        };
        if matches!(e.kind, EventKind::Gap | EventKind::RoomEnded) {
            self.pause(
                now,
                &e.scope,
                room,
                if matches!(e.kind, EventKind::Gap) {
                    "event_gap"
                } else {
                    "room_ended"
                },
                out,
            );
            return Ok(());
        }
        if room.paused {
            suppress(out, "scope_paused");
            return Ok(());
        }
        let fingerprint = serde_json::to_string(e)?;
        if let Some((_, previous)) = room.dedupe.get(&e.event_id) {
            if previous == &fingerprint {
                suppress(out, "duplicate_event");
            } else {
                self.pause(now, &e.scope, room, "event_id_conflict", out);
            }
            return Ok(());
        }
        if room.dedupe.len() + room.native_ids.len() >= self.config.max_dedupe_entries {
            self.pause(now, &e.scope, room, "dedupe_capacity_gap", out);
            return Ok(());
        }
        room.dedupe.insert(e.event_id.clone(), (now, fingerprint));
        let Some(user) = &e.user else {
            suppress(out, "missing_user");
            return Ok(());
        };
        let Some(id) = &user.id else {
            suppress(out, "missing_stable_user_id");
            return Ok(());
        };
        if e.evidence != Evidence::Verified {
            suppress(out, "unverified_input");
            return Ok(());
        }
        let nickname = clean_text(&user.nickname, 80);
        let nickname = if nickname.is_empty() {
            "朋友".to_owned()
        } else {
            nickname
        };
        match &e.kind {
            EventKind::Gift(_) if !self.config.gift_enabled => suppress(out, "rule_disabled"),
            EventKind::Gift(g) => self.observe_gift(now, e, g, id, &nickname, room, out)?,
            EventKind::SupporterEntered(title) => {
                if !self.config.supporter_welcome_enabled {
                    suppress(out, "rule_disabled");
                    return Ok(());
                }
                // Identity welcomes replace any still-waiting ordinary welcome.
                if let Some(window) = &mut room.welcome_window {
                    window.users.retain(|(user, _)| user != id);
                }
                let key = format!("supporter:{id}");
                if room
                    .welcomes
                    .get(&key)
                    .is_some_and(|at| now.saturating_sub(*at) < 60_000)
                {
                    suppress(out, "supporter_welcome_cooldown");
                    return Ok(());
                }
                if room.welcomes.len() >= 2_000 {
                    suppress(out, "welcome_capacity");
                    return Ok(());
                }
                if room.batches.contains_key(id)
                    || room
                        .pending
                        .values()
                        .any(|a| a.user_id == *id && a.priority > 20)
                {
                    suppress(out, "higher_priority_interaction");
                    return Ok(());
                }
                self.cancel_welcomes(now, &e.scope, id, room, out)?;
                let text = self.supporter_welcome.render(&HashMap::from([
                    ("nickname", clean_text(&nickname, 12)),
                    ("title", title.label().into()),
                ]))?;
                let action = self.action(
                    now,
                    &e.scope,
                    id,
                    ActionKind::Welcome,
                    text,
                    Evidence::Verified,
                    None,
                );
                if self.plan(action, room, out) {
                    room.welcomes.insert(key, now);
                }
            }
            EventKind::Welcome | EventKind::FansclubJoined => {
                if matches!(e.kind, EventKind::Welcome) && !self.config.welcome_enabled
                    || matches!(e.kind, EventKind::FansclubJoined)
                        && !self.config.fansclub_joined_enabled
                {
                    suppress(out, "rule_disabled");
                    return Ok(());
                }
                if room.batches.contains_key(id)
                    || room
                        .pending
                        .values()
                        .any(|a| a.user_id == *id && a.priority > 20)
                {
                    suppress(out, "higher_priority_interaction");
                    return Ok(());
                }
                let rule_key = format!(
                    "{}:{id}",
                    if matches!(e.kind, EventKind::Welcome) {
                        "welcome"
                    } else {
                        "fansclub_join"
                    }
                );
                if room
                    .welcomes
                    .get(&format!("supporter:{id}"))
                    .is_some_and(|at| now.saturating_sub(*at) < 60_000)
                    || room
                        .welcomes
                        .get(&rule_key)
                        .is_some_and(|at| now.saturating_sub(*at) < 60_000)
                {
                    suppress(out, "welcome_cooldown");
                    return Ok(());
                }
                if room.welcomes.len() >= 2_000 {
                    suppress(out, "welcome_capacity");
                    return Ok(());
                }
                if matches!(e.kind, EventKind::Welcome) {
                    let window = room.welcome_window.get_or_insert_with(|| WelcomeWindow {
                        due_at: now + WELCOME_WINDOW,
                        users: Vec::new(),
                    });
                    if window.users.len() >= self.config.max_active_users {
                        suppress(out, "welcome_capacity");
                        return Ok(());
                    }
                    window.users.push((id.clone(), nickname));
                    room.welcomes.insert(rule_key, now);
                    log(
                        out,
                        now,
                        &e.scope,
                        "accepted",
                        "welcome_batched",
                        Some(&e.event_id),
                        None,
                    );
                    return Ok(());
                }
                let text = self
                    .fansclub_joined
                    .render(&HashMap::from([("nickname", nickname)]))?;
                let action = self.action(
                    now,
                    &e.scope,
                    id,
                    ActionKind::Welcome,
                    text,
                    Evidence::Verified,
                    None,
                );
                if self.plan(action, room, out) {
                    room.welcomes.insert(rule_key, now);
                }
            }
            EventKind::LevelSnapshot(level) | EventKind::LevelUpgraded(level) => {
                let enabled = match level.kind.as_str() {
                    "fansclub" => self.config.fansclub_upgrade_enabled,
                    "honor" => self.config.honor_upgrade_enabled,
                    "wealth" => self.config.wealth_upgrade_enabled,
                    _ => true,
                };
                if !enabled {
                    suppress(out, "rule_disabled");
                    return Ok(());
                }
                if !matches!(level.kind.as_str(), "fansclub" | "honor" | "wealth") {
                    suppress(out, "unsupported_level_kind");
                    return Ok(());
                }
                let key = (id.clone(), level.kind.clone());
                if room.levels.len() >= 2_000 && !room.levels.contains_key(&key) {
                    suppress(out, "level_capacity");
                    return Ok(());
                }
                let snapshot = matches!(e.kind, EventKind::LevelSnapshot(_));
                let previous = room.levels.get(&key);
                let increasing =
                    previous.is_some_and(|b| b.source == e.source && level.new_level > b.value);
                let compatible = previous.is_none_or(|b| b.source == e.source);
                let same_or_down = previous.is_some_and(|b| level.new_level <= b.value);
                let decreasing = previous.is_some_and(|b| level.new_level < b.value);
                let baseline = Baseline {
                    value: level.new_level,
                    source: e.source.clone(),
                    at: now,
                };
                if compatible && (snapshot && same_or_down || !snapshot && decreasing) {
                    suppress(out, "level_not_increasing");
                    return Ok(());
                }
                room.levels.insert(key, baseline);
                if snapshot && (!increasing || !self.config.allow_inferred_levels) {
                    suppress(
                        out,
                        if !compatible {
                            "level_source_changed"
                        } else if increasing {
                            "inferred_levels_disabled"
                        } else {
                            "level_baseline_only"
                        },
                    );
                    return Ok(());
                }
                let congratulation = (id.clone(), level.kind.clone(), level.new_level);
                if room.congratulated.contains_key(&congratulation) {
                    suppress(out, "duplicate_level_upgrade");
                    return Ok(());
                }
                if room.congratulated.len() >= 2_000 {
                    suppress(out, "upgrade_capacity");
                    return Ok(());
                }
                self.cancel_welcomes(now, &e.scope, id, room, out)?;
                let text = self.upgrade.render(&HashMap::from([
                    ("nickname", nickname),
                    ("level_name", clean_text(&level.name, 64)),
                    ("new_level", level.new_level.to_string()),
                ]))?;
                let action = self.action(
                    now,
                    &e.scope,
                    id,
                    ActionKind::Upgrade,
                    text,
                    if snapshot {
                        Evidence::Inferred
                    } else {
                        Evidence::Verified
                    },
                    None,
                );
                if self.plan(action, room, out) {
                    room.congratulated.insert(congratulation, now);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn observe_gift(
        &mut self,
        now: u64,
        e: &CoreEvent,
        g: &Gift,
        user: &str,
        nickname: &str,
        room: &mut Room,
        out: &mut Vec<Audit>,
    ) -> Result<()> {
        if g.recipient_user_id != e.scope.recipient_user_id {
            log(
                out,
                now,
                &e.scope,
                "suppressed",
                "wrong_gift_recipient",
                Some(&e.event_id),
                None,
            );
            return Ok(());
        }
        if g.combo_end && g.quantity_mode != QuantityMode::Cumulative {
            log(
                out,
                now,
                &e.scope,
                "suppressed",
                "combo_end_without_increment",
                Some(&e.event_id),
                None,
            );
            return Ok(());
        }
        if g.quantity_mode != QuantityMode::Cumulative {
            let Some(native) = &e.native_event_id else {
                log(
                    out,
                    now,
                    &e.scope,
                    "suppressed",
                    "missing_native_gift_id",
                    Some(&e.event_id),
                    None,
                );
                return Ok(());
            };
            let native_key = (user.to_owned(), g.gift_id.clone(), native.clone());
            if room.native_ids.contains_key(&native_key) {
                log(
                    out,
                    now,
                    &e.scope,
                    "suppressed",
                    "duplicate_native_gift",
                    Some(&e.event_id),
                    None,
                );
                return Ok(());
            }
            if room.dedupe.len() + room.native_ids.len() >= self.config.max_dedupe_entries {
                self.pause(now, &e.scope, room, "dedupe_capacity_gap", out);
                return Ok(());
            }
            room.native_ids.insert(native_key, now);
        }
        let key = (user.to_owned(), g.gift_id.clone(), g.combo_id.clone());
        if room
            .groups
            .get(&key)
            .is_some_and(|group| group.mode != g.quantity_mode)
        {
            self.pause(now, &e.scope, room, "mixed_quantity_semantics", out);
            return Ok(());
        }
        if room.groups.len() >= 10_000 && !room.groups.contains_key(&key) {
            self.pause(now, &e.scope, room, "gift_group_capacity_gap", out);
            return Ok(());
        }
        let group = room.groups.entry(key).or_insert(Group {
            mode: g.quantity_mode,
            count: 0,
            seen_at: now,
        });
        group.seen_at = now;
        let delta = match g.quantity_mode {
            QuantityMode::Cumulative => {
                let current = g.quantity.expect("validated count");
                let delta = current.saturating_sub(group.count);
                group.count = group.count.max(current);
                if delta == 0 {
                    log(
                        out,
                        now,
                        &e.scope,
                        "suppressed",
                        "no_new_gift",
                        Some(&e.event_id),
                        None,
                    );
                    return Ok(());
                }
                Some(delta)
            }
            QuantityMode::Incremental => g.quantity,
            QuantityMode::Unknown => None,
        };
        if !room.batches.contains_key(user) && room.batches.len() >= self.config.max_active_users {
            self.pause(now, &e.scope, room, "active_user_capacity_gap", out);
            return Ok(());
        }
        if let Some(window) = room.batches.get(user) {
            if window.batch.event_ids.len() >= 1_000
                || window.batch.items.len() >= self.config.max_gift_items
                    && !window.batch.items.iter().any(|i| i.gift_id == g.gift_id)
            {
                self.pause(now, &e.scope, room, "gift_window_capacity_gap", out);
                return Ok(());
            }
        }
        self.cancel_welcomes(now, &e.scope, user, room, out)?;
        if !room.batches.contains_key(user) {
            self.batch_seq += 1;
            room.batches.insert(
                user.into(),
                Window {
                    batch: GiftBatch {
                        batch_id: format!("batch-{}", self.batch_seq),
                        revision: 0,
                        user_id: user.into(),
                        nickname: nickname.into(),
                        recipient_user_id: g.recipient_user_id.clone(),
                        last_gift_at: now,
                        due_at: now + SILENCE,
                        items: Vec::new(),
                        event_ids: Vec::new(),
                    },
                    planned: None,
                },
            );
        }
        let window = room.batches.get_mut(user).expect("inserted batch");
        if let Some(action_id) = window.planned.take() {
            if let Some(action) = room.pending.remove(&action_id) {
                log(
                    out,
                    now,
                    &e.scope,
                    "cancelled",
                    "new_gift",
                    None,
                    Some(action),
                );
            }
        }
        if let Some(item) = window
            .batch
            .items
            .iter_mut()
            .find(|i| i.gift_id == g.gift_id)
        {
            item.unit_value = item
                .unit_value
                .max(super::gift_text::valid_unit_value(g.unit_value));
            item.quantity = match (item.quantity, delta) {
                (Some(a), Some(b)) => match a.checked_add(b) {
                    Some(n) => Some(n),
                    None => {
                        self.pause(now, &e.scope, room, "gift_count_overflow", out);
                        return Ok(());
                    }
                },
                _ => None,
            };
        } else {
            window.batch.items.push(GiftItem {
                unit_value: super::gift_text::valid_unit_value(g.unit_value),
                gift_id: g.gift_id.clone(),
                name: clean_text(&g.name, 128),
                quantity: delta,
            });
        }
        window.batch.revision += 1;
        window.batch.nickname = nickname.into();
        window.batch.last_gift_at = now;
        window.batch.due_at = now + SILENCE;
        window.batch.event_ids.push(e.event_id.clone());
        log(
            out,
            now,
            &e.scope,
            "accepted",
            "new_gift",
            Some(&e.event_id),
            None,
        );
        Ok(())
    }

    fn cancel_welcomes(
        &mut self,
        now: u64,
        scope: &Scope,
        user: &str,
        room: &mut Room,
        out: &mut Vec<Audit>,
    ) -> Result<()> {
        if let Some(window) = &mut room.welcome_window {
            let before = window.users.len();
            window.users.retain(|(id, _)| id != user);
            if before != window.users.len() {
                log(
                    out,
                    now,
                    scope,
                    "cancelled",
                    "higher_priority_interaction",
                    None,
                    None,
                );
            }
        }
        let affected: Vec<_> = room
            .welcome_actions
            .iter()
            .filter(|(_, users)| users.iter().any(|(id, _)| id == user))
            .map(|(id, _)| id.clone())
            .collect();
        for id in affected {
            let mut users = room.welcome_actions.remove(&id).expect("welcome members");
            if let Some(old) = room.pending.remove(&id) {
                log(
                    out,
                    now,
                    scope,
                    "cancelled",
                    "higher_priority_interaction",
                    None,
                    Some(old.clone()),
                );
                users.retain(|(id, _)| id != user);
                if !users.is_empty() {
                    let mut action = self.action(
                        now,
                        scope,
                        &users[0].0,
                        ActionKind::Welcome,
                        self.welcome_text(&users)?,
                        Evidence::Verified,
                        None,
                    );
                    action.expires_at = old.expires_at;
                    if self.plan(action.clone(), room, out) {
                        room.welcome_actions.insert(action.action_id, users);
                    }
                }
            }
        }
        let ids: Vec<_> = room
            .pending
            .values()
            .filter(|a| a.user_id == user && a.kind == ActionKind::Welcome)
            .map(|a| a.action_id.clone())
            .collect();
        for id in ids {
            let action = room.pending.remove(&id).expect("pending welcome");
            log(
                out,
                now,
                scope,
                "cancelled",
                "higher_priority_interaction",
                None,
                Some(action),
            );
        }
        Ok(())
    }

    fn welcome_text(&self, users: &[(String, String)]) -> Result<String> {
        super::welcome::render(&self.welcome, users, self.config.max_text_chars)
    }

    fn gift_text(&self, batch: &GiftBatch) -> Result<String> {
        // At most 100 gift items: their u64 quantities fit exactly in u128.
        let mut names: Vec<(String, Option<u128>)> = Vec::new();
        for item in &batch.items {
            if let Some((_, count)) = names.iter_mut().find(|(name, _)| name == &item.name) {
                *count = match (*count, item.quantity) {
                    (Some(a), Some(b)) => Some(a + u128::from(b)),
                    _ => None,
                };
            } else {
                names.push((item.name.clone(), item.quantity.map(u128::from)));
            }
        }
        let unit_value = batch.items.iter().filter_map(|i| i.unit_value).max();
        let nickname = clean_text(
            &batch.nickname,
            if self.config.gift_show_quantity {
                80
            } else {
                12
            },
        );
        if names.iter().map(|(n, _)| n.chars().count()).sum::<usize>() > 30 {
            return self.gift_text.render_for(
                &batch.user_id,
                nickname,
                String::new(),
                true,
                unit_value,
            );
        }
        let list = names
            .into_iter()
            .map(
                |(n, count)| match count.filter(|_| self.config.gift_show_quantity) {
                    Some(count) => format!("{n}×{count}"),
                    None => n,
                },
            )
            .collect::<Vec<_>>()
            .join("、");
        self.gift_text
            .render_for(&batch.user_id, nickname, list, false, unit_value)
    }

    fn action(
        &mut self,
        at: u64,
        scope: &Scope,
        user: &str,
        kind: ActionKind,
        text: String,
        evidence: Evidence,
        gift_batch: Option<GiftBatch>,
    ) -> PreviewAction {
        self.action_seq += 1;
        let (priority, ttl) = match kind {
            ActionKind::Welcome => (20, 10_000),
            ActionKind::GiftThanks => (70, 20_000),
            ActionKind::Upgrade => (80, 20_000),
        };
        PreviewAction {
            action_id: format!("action-{}", self.action_seq),
            mode: "preview".into(),
            scope: scope.clone(),
            user_id: user.into(),
            kind,
            priority,
            text,
            created_at: at,
            expires_at: at + ttl,
            evidence,
            gift_batch,
        }
    }

    fn plan(&self, action: PreviewAction, room: &mut Room, out: &mut Vec<Audit>) -> bool {
        if action.text.chars().count() > self.config.max_text_chars {
            log(
                out,
                action.created_at,
                &action.scope,
                "suppressed",
                "text_too_long",
                None,
                Some(action.clone()),
            );
            return false;
        }
        if room.pending.len() >= self.config.max_pending_actions {
            let victim = room
                .pending
                .values()
                .filter(|a| a.kind == ActionKind::Welcome && a.priority < action.priority)
                .min_by_key(|a| (a.created_at, &a.action_id))
                .map(|a| a.action_id.clone());
            if let Some(id) = victim {
                let old = room.pending.remove(&id).expect("pending");
                log(
                    out,
                    action.created_at,
                    &action.scope,
                    "cancelled",
                    "queue_priority",
                    None,
                    Some(old),
                );
            } else {
                log(
                    out,
                    action.created_at,
                    &action.scope,
                    "suppressed",
                    "queue_full",
                    None,
                    Some(action.clone()),
                );
                return false;
            }
        }
        log(
            out,
            action.created_at,
            &action.scope,
            "planned",
            "offline_preview",
            None,
            Some(action.clone()),
        );
        room.pending.insert(action.action_id.clone(), action);
        true
    }

    /// Stop pending interactions without clearing dedupe or safety pauses.
    pub fn cancel_pending(&mut self, now: u64) -> Result<Vec<Audit>> {
        self.check_clock(now)?;
        self.clock = now;
        let mut out = vec![];
        for (scope, room) in &mut self.rooms {
            for (_, action) in std::mem::take(&mut room.pending) {
                log(
                    &mut out,
                    now,
                    scope,
                    "cancelled",
                    "moderation_disabled",
                    None,
                    Some(action),
                );
            }
            room.batches.clear();
            room.welcome_window = None;
            room.welcome_actions.clear();
            room.groups.clear();
        }
        Ok(out)
    }
    /// Observation sinks can complete previews without ever invoking an executor.
    pub fn has_pending_work(&self) -> bool {
        self.rooms.values().any(|r| {
            !r.batches.is_empty()
                || !r.pending.is_empty()
                || r.welcome_window
                    .as_ref()
                    .is_some_and(|w| !w.users.is_empty())
        })
    }
    /// A collecting welcome window has no command to send yet. It must not
    /// indefinitely starve timed messages in rooms with continuous arrivals.
    pub fn has_pending_dispatch(&self) -> bool {
        self.rooms
            .values()
            .any(|r| !r.batches.is_empty() || !r.pending.is_empty())
    }
    pub fn pending_action_ids(&self) -> Vec<String> {
        self.rooms
            .values()
            .flat_map(|room| room.pending.keys().cloned())
            .collect()
    }

    /// The rule engine owns the queue; schedulers never retain stale copies.
    pub fn pending_actions(&self) -> Vec<&PreviewAction> {
        self.rooms
            .values()
            .flat_map(|room| room.pending.values())
            .collect()
    }

    /// Discard only the affected incomplete gift window, retaining room dedupe.
    pub fn discard_user_gifts(
        &mut self,
        now: u64,
        scope: &Scope,
        user: &str,
        reason: &str,
    ) -> Result<Vec<Audit>> {
        self.check_clock(now)?;
        self.clock = now;
        let mut out = Vec::new();
        if let Some(room) = self.rooms.get_mut(scope) {
            if let Some(window) = room.batches.remove(user) {
                let action = window.planned.and_then(|id| room.pending.remove(&id));
                log(&mut out, now, scope, "cancelled", reason, None, action);
            }
        }
        Ok(out)
    }

    /// Models the freeze boundary only, never sends or retries a command.
    /// Used by offline fixtures and observation sinks to finalize their previews.
    pub fn simulate_dispatch(&mut self, now: u64, action_id: &str) -> Result<Vec<Audit>> {
        let mut out = self.advance(now)?;
        let scope = self
            .rooms
            .iter()
            .find(|(_, r)| r.pending.contains_key(action_id))
            .map(|(s, _)| s.clone())
            .ok_or_else(|| anyhow::anyhow!("unknown pending action"))?;
        let room = self.rooms.get_mut(&scope).expect("existing scope");
        let action = room.pending.remove(action_id).expect("pending");
        if room
            .batches
            .get(&action.user_id)
            .is_some_and(|w| w.planned.as_deref() == Some(action_id))
        {
            room.batches.remove(&action.user_id);
        }
        log(
            &mut out,
            now,
            &scope,
            "dispatch_simulated",
            "frozen_without_execution",
            None,
            Some(action),
        );
        Ok(out)
    }
}
