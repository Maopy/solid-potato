use anyhow::{Result, bail};
use std::collections::HashMap;

#[derive(Clone, Debug)]
enum Part {
    Literal(String),
    Variable(String),
}

#[derive(Clone, Debug)]
pub struct Template(Vec<Part>);

impl Template {
    pub fn parse(value: &str, allowed: &[&str]) -> Result<Self> {
        if value.is_empty() || value.chars().count() > 500 || value.chars().any(char::is_control) {
            bail!("invalid template length or controls");
        }
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut chars = value.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' => {
                    if chars.peek() == Some(&'{') {
                        chars.next();
                        literal.push('{');
                        continue;
                    }
                    if !literal.is_empty() {
                        parts.push(Part::Literal(std::mem::take(&mut literal)));
                    }
                    let mut name = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some(c) if c != '{' => name.push(c),
                            _ => bail!("unclosed template variable"),
                        }
                    }
                    if !allowed.contains(&name.as_str()) {
                        bail!("unknown template variable: {name}");
                    }
                    parts.push(Part::Variable(name));
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    literal.push('}');
                }
                '}' => bail!("unmatched template brace"),
                _ => literal.push(c),
            }
        }
        if !literal.is_empty() {
            parts.push(Part::Literal(literal));
        }
        Ok(Self(parts))
    }

    pub fn render(&self, values: &HashMap<&str, String>) -> Result<String> {
        let mut result = String::new();
        for part in &self.0 {
            match part {
                Part::Literal(value) => result.push_str(value),
                Part::Variable(name) => result.push_str(
                    values
                        .get(name.as_str())
                        .ok_or_else(|| anyhow::anyhow!("missing template value: {name}"))?,
                ),
            }
        }
        Ok(result)
    }
}
