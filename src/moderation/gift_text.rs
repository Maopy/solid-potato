//! Shared gift wording. Choose the suffix once when creating a draft, then
//! persist that text unchanged through queueing and receipt retries.
use super::templates::Template;
use anyhow::{Result, bail};
use serde::Deserialize;
use std::{collections::HashMap, sync::Mutex};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GiftVariation {
    pub enabled: bool,
    pub tiers: Option<Vec<ValueTier>>,
    pub unknown_templates: Option<Vec<String>>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueTier {
    pub min_unit_value: u64,
    pub templates: Vec<String>,
}

pub const MAX_UNIT_VALUE: u64 = 1_000_000_000;
pub fn valid_unit_value(value: Option<u64>) -> Option<u64> {
    value.filter(|v| *v <= MAX_UNIT_VALUE)
}

pub fn default_emojis() -> Vec<String> {
    ["🎁", "🎉", "💖", "✨", "🌟", "💐"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub struct GiftText {
    nicknames: super::nicknames::Nicknames,
    normal: Template,
    overflow: Template,
    emojis: Vec<String>,
    max_chars: usize,
    pools: Vec<(u64, Vec<(String, Template)>)>,
    unknown: Vec<(String, Template)>,
    previous: Mutex<HashMap<String, String>>,
}
impl GiftText {
    pub fn new(normal: &str, overflow: &str, emojis: &[String], max_chars: usize) -> Result<Self> {
        if emojis.len() > 32
            || emojis.iter().any(|e| {
                e.trim().is_empty() || e.chars().count() > 16 || e.chars().any(char::is_control)
            })
        {
            bail!(
                "gift.emojis requires at most 32 nonempty suffixes, each <=16 characters without controls"
            );
        }
        Ok(Self {
            nicknames: Default::default(),
            normal: Template::parse(normal, &["nickname", "gift_list"])?,
            overflow: Template::parse(overflow, &["nickname"])?,
            emojis: emojis.to_vec(),
            max_chars,
            pools: vec![],
            unknown: vec![],
            previous: Mutex::new(HashMap::new()),
        })
    }
    pub fn with_nicknames(mut self, users: &super::nicknames::Nicknames) -> Result<Self> {
        super::nicknames::validate(users)?;
        self.nicknames = users
            .iter()
            .map(|(id, name)| (id.clone(), name.trim().to_owned()))
            .collect();
        Ok(self)
    }
    pub fn render(&self, nickname: String, gift_list: String, overflow: bool) -> Result<String> {
        self.render_for("", nickname, gift_list, overflow, None)
    }
    pub fn render_for(
        &self,
        user: &str,
        nickname: String,
        gift_list: String,
        overflow: bool,
        unit_value: Option<u64>,
    ) -> Result<String> {
        let nickname = self.nicknames.get(user).cloned().unwrap_or(nickname);
        let values = HashMap::from([("nickname", nickname), ("gift_list", gift_list)]);
        let suffix = if self.emojis.is_empty() {
            ""
        } else {
            &self.emojis[(uuid::Uuid::new_v4().as_u128() % self.emojis.len() as u128) as usize]
        };
        let mut text = if overflow {
            self.overflow.render(&values)?
        } else {
            let pool = valid_unit_value(unit_value)
                .and_then(|v| {
                    self.pools
                        .iter()
                        .rev()
                        .find(|(min, _)| v >= *min)
                        .map(|(_, p)| p)
                })
                .unwrap_or(&self.unknown);
            if pool.is_empty() {
                self.normal.render(&values)?
            } else {
                let mut previous = self
                    .previous
                    .lock()
                    .map_err(|_| anyhow::anyhow!("gift wording lock failed"))?;
                let candidates: Vec<_> = pool
                    .iter()
                    .filter(|(source, _)| {
                        previous.get(user) != Some(source) && previous.get("") != Some(source)
                    })
                    .collect();
                let mut choices = if candidates.is_empty() {
                    pool.iter()
                        .filter(|(source, _)| previous.get(user) != Some(source))
                        .collect::<Vec<_>>()
                } else {
                    candidates
                };
                if choices.is_empty() {
                    choices = pool.iter().collect();
                }
                let (source, template) =
                    choices[(uuid::Uuid::new_v4().as_u128() % choices.len() as u128) as usize];
                if previous.len() >= 10000 && !previous.contains_key(user) {
                    if let Some(key) = previous.keys().next().cloned() {
                        previous.remove(&key);
                    }
                }
                previous.insert(user.into(), source.clone());
                previous.insert(String::new(), source.clone());
                template.render(&values)?
            }
        };
        // Keep the complete suffix (including ZWJ sequences). If the normal
        // wording no longer fits, try the configured overflow wording.
        if !suffix.is_empty() && text.chars().count() + suffix.chars().count() > self.max_chars {
            text = self.overflow.render(&values)?;
        }
        text.push_str(suffix);
        Ok(text)
    }
}

impl GiftText {
    pub fn with_variation(mut self, config: &GiftVariation) -> Result<Self> {
        let tiers = config.tiers.clone().unwrap_or_else(default_tiers);
        let unknown = config
            .unknown_templates
            .clone()
            .unwrap_or_else(default_unknown_templates);
        let parse = |sources: &[String]| -> Result<Vec<(String, Template)>> {
            if sources.is_empty() || sources.len() > 64 {
                bail!("gift variation pools require 1..64 templates");
            }
            let mut result = vec![];
            for source in sources {
                if source.trim().is_empty()
                    || source.chars().count() > 512
                    || source.chars().any(char::is_control)
                {
                    bail!("invalid gift variation template");
                }
                if result.iter().any(|(s, _)| s == source) {
                    bail!("duplicate gift variation template");
                }
                result.push((
                    source.clone(),
                    Template::parse(source, &["nickname", "gift_list"])?,
                ));
            }
            Ok(result)
        };
        if tiers.is_empty()
            || tiers.len() > 16
            || tiers[0].min_unit_value != 0
            || tiers
                .windows(2)
                .any(|p| p[0].min_unit_value >= p[1].min_unit_value)
            || tiers.iter().any(|t| t.min_unit_value > MAX_UNIT_VALUE)
        {
            bail!(
                "gift variation tiers must start at zero and increase strictly, at most 16 tiers"
            );
        }
        let mut pools = vec![];
        for tier in tiers {
            pools.push((tier.min_unit_value, parse(&tier.templates)?));
        }
        let unknown = parse(&unknown)?;
        if config.enabled {
            self.pools = pools;
            self.unknown = unknown;
        }
        Ok(self)
    }
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}
pub fn default_unknown_templates() -> Vec<String> {
    strings(&[
        "霸气！感谢{nickname}的{gift_list}",
        "{nickname}太给力了！感谢{gift_list}",
        "漂亮！谢谢{nickname}送来的{gift_list}",
        "感谢{nickname}的{gift_list}，气氛直接拉满",
        "{nickname}这一手帅气！感谢{gift_list}",
        "感谢{nickname}送上{gift_list}，一起燃起来",
    ])
}
pub fn default_tiers() -> Vec<ValueTier> {
    vec![
        ValueTier {
            min_unit_value: 0,
            templates: strings(&[
                "感谢{nickname}的{gift_list}，给力！",
                "{nickname}帅气！谢谢你的{gift_list}",
                "漂亮！感谢{nickname}送来的{gift_list}",
                "感谢{nickname}的{gift_list}，气氛起来了",
                "{nickname}太棒了！感谢{gift_list}",
                "谢谢{nickname}送上{gift_list}，一起嗨起来",
                "感谢{nickname}的{gift_list}，这波很赞",
                "{nickname}有排面！感谢你的{gift_list}",
            ]),
        },
        ValueTier {
            min_unit_value: 100,
            templates: strings(&[
                "霸气！感谢{nickname}送来的{gift_list}",
                "{nickname}这波太帅了！感谢{gift_list}",
                "感谢{nickname}的{gift_list}，排面安排上了",
                "{nickname}太给力了！{gift_list}把气氛拉满",
                "漂亮的一手！感谢{nickname}的{gift_list}",
                "感谢{nickname}送上{gift_list}，这波够燃",
                "{nickname}帅气出手！谢谢你的{gift_list}",
                "感谢{nickname}的{gift_list}，气势直接拉起来",
            ]),
        },
        ValueTier {
            min_unit_value: 1000,
            templates: strings(&[
                "太霸气了！感谢{nickname}的{gift_list}，排面拉满",
                "{nickname}这波太炸了！感谢{gift_list}",
                "感谢{nickname}送来的{gift_list}，直接燃爆了",
                "{nickname}出手就是排面！感谢你的{gift_list}",
                "霸气十足！感谢{nickname}送上{gift_list}",
                "感谢{nickname}的{gift_list}，这波帅到飞起",
                "{nickname}太猛了！感谢{gift_list}，气势拉满",
                "感谢{nickname}的{gift_list}，这一手真的炸裂",
            ]),
        },
        ValueTier {
            min_unit_value: 10000,
            templates: strings(&[
                "霸气冲天！感谢{nickname}的{gift_list}，顶级排面",
                "{nickname}这波封神了！感谢{gift_list}",
                "感谢{nickname}送来的{gift_list}，这排面直接拉爆",
                "{nickname}王者出手！感谢{gift_list}，气势全开",
                "太炸裂了！感谢{nickname}的{gift_list}，燃到顶了",
                "感谢{nickname}的{gift_list}，霸气到没朋友",
                "{nickname}这一手震撼全场！感谢{gift_list}",
                "感谢{nickname}送上{gift_list}，这波排面天花板",
            ]),
        },
    ]
}
