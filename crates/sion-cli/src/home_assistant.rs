//! Home Assistant's devices as tools, through its REST API with a
//! long-lived access token: what the home's entities are and what state
//! each is in (`ha_states`, `ha_state`), what services it offers
//! (`ha_services`), and calling one (`ha_call`: a light on, a thermostat
//! set). `HOME_ASSISTANT_URL` names the instance
//! (`http://homeassistant.local:8123`, say) and `HOME_ASSISTANT_TOKEN`
//! holds the token, made under the user's profile in Home Assistant.
//!
//! Reading is a read; calling a service acts in the home, so it is an
//! external call, which autonomy L4 asks a person for and L5 runs. What
//! Home Assistant says (names its users gave) is from outside the agent's
//! trust, as a web page is.

use std::time::Duration;

use serde_json::{json, Value};
use sion_agent::tools::{Effect, Outcome, Tool};
use sion_agent::ToolSpec;

use crate::error::CliError;

/// The most lines a listing gives.
const MOST_LINES: usize = 200;

/// Home Assistant's API: where, and with what token.
#[derive(Clone)]
pub struct HomeAssistant {
    client: sion_http::Client,
    base: String,
    token: String,
}

impl std::fmt::Debug for HomeAssistant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HomeAssistant({})", self.base)
    }
}

/// Whether `name` is a Home Assistant name (a domain, a service, an
/// entity's object id): lower-case letters, digits and `_`.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Whether `entity` is an entity id (`light.kitchen`).
fn entity_id(entity: &str) -> bool {
    entity
        .split_once('.')
        .is_some_and(|(domain, object)| plain(domain) && plain(object))
}

/// An entity's line: its id, its state, and its name.
fn state_line(state: &Value) -> Option<String> {
    let id = state["entity_id"].as_str()?;
    let value = state["state"].as_str().unwrap_or("?");
    let unit = state["attributes"]["unit_of_measurement"]
        .as_str()
        .map(|u| format!(" {u}"))
        .unwrap_or_default();
    Some(match state["attributes"]["friendly_name"].as_str() {
        Some(name) => format!("{id}: {value}{unit} ({name})"),
        None => format!("{id}: {value}{unit}"),
    })
}

impl HomeAssistant {
    /// The instance `HOME_ASSISTANT_URL` names, with the token in
    /// `HOME_ASSISTANT_TOKEN`.
    pub fn from_env() -> Result<HomeAssistant, CliError> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let base = env("HOME_ASSISTANT_URL").ok_or_else(|| {
            CliError::Usage("set HOME_ASSISTANT_URL (http://homeassistant.local:8123, say)".into())
        })?;
        let token = env("HOME_ASSISTANT_TOKEN").ok_or_else(|| {
            CliError::Usage("set HOME_ASSISTANT_TOKEN (a long-lived access token)".into())
        })?;
        let client = sion_http::Client::new(sion_http::Options {
            timeout: Duration::from_secs(30),
            // A home's instance is on its own network: the person names it.
            allow_private: true,
            ..sion_http::Options::default()
        })
        .map_err(|e| CliError::Usage(e.to_string()))?;
        Ok(HomeAssistant {
            client,
            base: base.trim_end_matches('/').to_string(),
            token,
        })
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let auth = format!("Bearer {}", self.token);
        let bytes = body.map(|b| b.to_string().into_bytes());
        let mut headers = vec![("Authorization", auth.as_str())];
        if bytes.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        let fetched = self
            .client
            .request(
                method,
                &format!("{}/api{path}", self.base),
                &headers,
                bytes.as_deref(),
                8 << 20,
            )
            .map_err(|e| e.to_string().replace(&self.token, "<token>"))?;
        serde_json::from_slice(&fetched.body)
            .map_err(|e| format!("Home Assistant answered with something other than JSON: {e}"))
    }

    /// The four tools.
    pub fn tools(&self) -> Vec<HomeTool> {
        ["ha_states", "ha_state", "ha_services", "ha_call"]
            .into_iter()
            .map(|name| HomeTool {
                home: self.clone(),
                name,
            })
            .collect()
    }
}

/// One of Home Assistant's tools.
#[derive(Debug)]
pub struct HomeTool {
    home: HomeAssistant,
    name: &'static str,
}

impl Tool for HomeTool {
    fn spec(&self) -> ToolSpec {
        let (description, schema) = match self.name {
            "ha_states" => (
                "The home's entities and their states, from Home Assistant: one a line, `entity_id: state (name)`. Give a domain (light, switch, climate, sensor…) or words to look for, to see fewer.",
                json!({"type": "object", "properties": {
                    "domain": {"type": "string", "description": "only this domain's entities"},
                    "search": {"type": "string", "description": "only entities whose id or name has these words"}}}),
            ),
            "ha_state" => (
                "One entity's state and attributes, from Home Assistant.",
                json!({"type": "object", "properties": {
                    "entity_id": {"type": "string", "description": "light.kitchen, say"}},
                    "required": ["entity_id"]}),
            ),
            "ha_services" => (
                "The services Home Assistant offers for a domain (light, climate, media_player…), with the fields each takes.",
                json!({"type": "object", "properties": {
                    "domain": {"type": "string"}}, "required": ["domain"]}),
            ),
            _ => (
                "Call a Home Assistant service: act in the home (turn a light on, set a thermostat). It says which entities changed.",
                json!({"type": "object", "properties": {
                    "domain": {"type": "string", "description": "light, say"},
                    "service": {"type": "string", "description": "turn_on, say"},
                    "data": {"type": "object", "description": "the service's fields: entity_id, brightness_pct…"}},
                    "required": ["domain", "service"]}),
            ),
        };
        ToolSpec {
            name: self.name.into(),
            description: description.into(),
            input_schema: schema,
        }
    }

    fn effect(&self) -> Effect {
        if self.name == "ha_call" {
            Effect::External
        } else {
            Effect::Read
        }
    }

    fn journaled(&self) -> bool {
        self.name == "ha_call"
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let outcome = match self.name {
            "ha_states" => self.states(input),
            "ha_state" => self.state(input),
            "ha_services" => self.services(input),
            _ => self.act(input),
        };
        match outcome {
            Ok(text) => Outcome::ok(text).from_outside(),
            Err(why) => Outcome::error(why),
        }
    }
}

impl HomeTool {
    fn states(&self, input: &Value) -> Result<String, String> {
        let states = self.home.call("GET", "/states", None)?;
        let domain = input["domain"].as_str().map(|d| format!("{d}."));
        let words: Vec<String> = input["search"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let lines: Vec<String> = states
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| {
                let id = s["entity_id"].as_str().unwrap_or_default();
                domain.as_deref().map_or(true, |d| id.starts_with(d))
            })
            .filter_map(state_line)
            .filter(|line| {
                let line = line.to_lowercase();
                words.iter().all(|w| line.contains(w.as_str()))
            })
            .collect();
        if lines.is_empty() {
            return Ok("no entity matches".into());
        }
        let more = lines.len().saturating_sub(MOST_LINES);
        let mut text = lines
            .into_iter()
            .take(MOST_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        if more > 0 {
            text.push_str(&format!(
                "\n({more} more: give a domain or words to see fewer)"
            ));
        }
        Ok(text)
    }

    fn state(&self, input: &Value) -> Result<String, String> {
        let entity = input["entity_id"].as_str().unwrap_or_default();
        if !entity_id(entity) {
            return Err(format!(
                "{entity:?} is not an entity id (light.kitchen, say)"
            ));
        }
        let state = self.home.call("GET", &format!("/states/{entity}"), None)?;
        Ok(state.to_string())
    }

    fn services(&self, input: &Value) -> Result<String, String> {
        let domain = input["domain"].as_str().unwrap_or_default();
        if !plain(domain) {
            return Err(format!("{domain:?} is not a domain (light, say)"));
        }
        let services = self.home.call("GET", "/services", None)?;
        let found = services
            .as_array()
            .into_iter()
            .flatten()
            .find(|d| d["domain"] == domain)
            .ok_or_else(|| format!("Home Assistant has no domain {domain}"))?;
        let lines: Vec<String> = found["services"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, service)| {
                let fields: Vec<&str> = service["fields"]
                    .as_object()
                    .map(|f| f.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                format!(
                    "{domain}.{name}: {}{}",
                    service["description"].as_str().unwrap_or_default(),
                    if fields.is_empty() {
                        String::new()
                    } else {
                        format!(" (fields: {})", fields.join(", "))
                    }
                )
            })
            .collect();
        Ok(lines.join("\n"))
    }

    fn act(&self, input: &Value) -> Result<String, String> {
        let (domain, service) = (
            input["domain"].as_str().unwrap_or_default(),
            input["service"].as_str().unwrap_or_default(),
        );
        if !plain(domain) || !plain(service) {
            return Err("give a domain and a service (light, turn_on)".into());
        }
        let data = match &input["data"] {
            Value::Null => json!({}),
            Value::Object(_) => input["data"].clone(),
            _ => return Err("data is an object of the service's fields".into()),
        };
        let changed = self.home.call(
            "POST",
            &format!("/services/{domain}/{service}"),
            Some(&data),
        )?;
        let lines: Vec<String> = changed
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(state_line)
            .collect();
        Ok(if lines.is_empty() {
            format!("called {domain}.{service}: no entity changed state")
        } else {
            format!("called {domain}.{service}; now:\n{}", lines.join("\n"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_lines_are_home_assistants() {
        assert!(entity_id("light.kitchen_2") && !entity_id("light") && !entity_id("light.Kitchen"));
        assert!(!entity_id("../api.x") && plain("turn_on") && !plain("turn on"));
        let state = json!({"entity_id": "sensor.hall", "state": "21.5",
            "attributes": {"friendly_name": "Hall", "unit_of_measurement": "°C"}});
        assert_eq!(state_line(&state).unwrap(), "sensor.hall: 21.5 °C (Hall)");
    }
}
