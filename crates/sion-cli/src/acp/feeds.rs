//! Product feeds: what a merchant sells, published to an agent so it can
//! find it before it checks out.
//!
//! ACP's Feed API is hosted by the *agent*; merchants call it, never the
//! other way round. Both sides are here:
//!
//! - [`FeedHost`] is the agent's Feed API (API version 2026-04-17): `POST
//!   /feeds` makes a feed's metadata, `GET /feeds/{id}` and `GET
//!   /feeds/{id}/products` read back what it knows, and `PATCH
//!   /feeds/{id}/products` upserts products by id (products left out are
//!   left as they were). Products are checked against ACP's feed schema (a
//!   product has an id and variants; a variant an id and a title; prices are
//!   integer minor units). Feeds are kept across restarts, and an agent can
//!   search them ([`FeedHost::search`]).
//! - A merchant's catalog becomes products ([`products_of`]): one product
//!   per offer, one variant whose id is the checkout item id, as ACP asks
//!   ("ensure the feed `Variant.id` is accepted as an ACP checkout item
//!   ID"). It is published with [`publish`] (create the feed if needed,
//!   then upsert), or written as ACP's offline snapshot ([`export`]:
//!   `metadata.json` and `products.jsonl`).
//!
//! ACP's discovery document has no `feeds` service in this API version (its
//! list is closed per version), so a merchant does not advertise it there;
//! which feed to fill is agreed out of band, as ACP says for now.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::http::{self, Gate, Keys, Request, Response, Service};
use super::merchant::Catalog;
use super::state::Store;
use crate::error::CliError;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Feed {
    target_country: Option<String>,
    updated_at: u64,
    products: BTreeMap<String, Value>,
}

#[derive(Default, Serialize, Deserialize)]
struct Book {
    feeds: BTreeMap<String, Feed>,
    keys: Keys,
}

/// An agent's Feed API.
pub struct FeedHost {
    gate: Gate,
    store: Option<Store>,
    book: Mutex<Book>,
}

impl std::fmt::Debug for FeedHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "FeedHost({:?})",
            self.store.as_ref().map(|s| s.path().to_path_buf())
        )
    }
}

/// Checks a product against ACP's feed schema: an id, at least one
/// variant, each with an id and a title, prices in integer minor units.
pub fn check_product(product: &Value, at: &str) -> Result<(), (String, String)> {
    if product["id"].as_str().map_or(true, str::is_empty) {
        return Err(("a product has an id".into(), format!("{at}.id")));
    }
    let variants = product["variants"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            (
                "a product has at least one variant".to_string(),
                format!("{at}.variants"),
            )
        })?;
    for (index, variant) in variants.iter().enumerate() {
        let path = format!("{at}.variants[{index}]");
        if variant["id"].as_str().map_or(true, str::is_empty) {
            return Err(("a variant has an id".into(), format!("{path}.id")));
        }
        if variant["title"].as_str().map_or(true, str::is_empty) {
            return Err(("a variant has a title".into(), format!("{path}.title")));
        }
        for price in ["price", "list_price"] {
            let given = &variant[price];
            if !given.is_null() && !(given["amount"].is_u64() && given["currency"].is_string()) {
                return Err((
                    format!("a variant's {price} is {{amount: minor units, currency}}"),
                    format!("{path}.{price}"),
                ));
            }
        }
    }
    Ok(())
}

impl FeedHost {
    /// A Feed API for `clients` (the merchants and seller platforms that
    /// publish to it), kept in `store` if one is given.
    pub fn new(clients: Vec<http::Client>, store: Option<Store>) -> Result<FeedHost, CliError> {
        let mut book: Book = match &store {
            Some(store) => store.load()?,
            None => Book::default(),
        };
        http::release_running(&mut book.keys);
        Ok(FeedHost {
            gate: Gate::new(clients, &[super::API_VERSION])?,
            store,
            book: Mutex::new(book),
        })
    }

    fn save(&self, book: &Book) -> Result<(), Response> {
        match &self.store {
            Some(store) => store.save(book).map_err(|e| {
                http::error(
                    503,
                    "service_unavailable",
                    "state_not_saved",
                    &e.to_string(),
                    None,
                )
            }),
            None => Ok(()),
        }
    }

    fn metadata(id: &str, feed: &Feed) -> Value {
        let mut metadata = json!({"id": id, "updated_at": http::rfc3339(feed.updated_at)});
        if let Some(country) = &feed.target_country {
            metadata["target_country"] = json!(country);
        }
        metadata
    }

    /// The products of every feed whose title, description or variants
    /// name every word of `query` (no case), best first: what an agent
    /// finds before it checks out.
    pub fn search(&self, query: &str) -> Vec<(String, Value)> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let book = self.book.lock().expect("feeds lock");
        let mut found: Vec<(usize, String, Value)> = Vec::new();
        for (feed, data) in &book.feeds {
            for product in data.products.values() {
                let text = product.to_string().to_lowercase();
                if words.iter().all(|w| text.contains(w.as_str())) {
                    let in_title = words
                        .iter()
                        .filter(|w| {
                            product["title"]
                                .as_str()
                                .is_some_and(|t| t.to_lowercase().contains(w.as_str()))
                        })
                        .count();
                    found.push((in_title, feed.clone(), product.clone()));
                }
            }
        }
        found.sort_by_key(|f| std::cmp::Reverse(f.0));
        found.into_iter().map(|(_, feed, p)| (feed, p)).collect()
    }

    fn create(&self, body: &Value) -> Response {
        let country = body.get("target_country").filter(|c| !c.is_null());
        if let Some(country) = country {
            if country.as_str().map_or(true, |c| c.len() != 2) {
                return http::invalid(
                    "invalid",
                    "target_country is ISO 3166-1 alpha-2",
                    Some("$.target_country"),
                );
            }
        }
        let id = http::fresh_id("feed");
        let feed = Feed {
            target_country: country.and_then(Value::as_str).map(str::to_string),
            updated_at: http::now(),
            products: BTreeMap::new(),
        };
        let mut book = self.book.lock().expect("feeds lock");
        let metadata = Self::metadata(&id, &feed);
        book.feeds.insert(id, feed);
        if let Err(refused) = self.save(&book) {
            return refused;
        }
        Response::json(201, metadata)
    }

    fn upsert(&self, id: &str, body: &Value) -> Response {
        let Some(products) = body["products"].as_array() else {
            return http::invalid("invalid", "products is a list", Some("$.products"));
        };
        for (index, product) in products.iter().enumerate() {
            if let Err((message, param)) = check_product(product, &format!("$.products[{index}]")) {
                return http::invalid("invalid", &message, Some(&param));
            }
        }
        let mut book = self.book.lock().expect("feeds lock");
        let Some(feed) = book.feeds.get_mut(id) else {
            return http::error(
                404,
                "invalid_request",
                "feed_not_found",
                "Feed not found",
                Some("id"),
            );
        };
        for product in products {
            let key = product["id"].as_str().unwrap_or_default().to_string();
            feed.products.insert(key, product.clone());
        }
        feed.updated_at = http::now();
        if let Err(refused) = self.save(&book) {
            return refused;
        }
        Response::json(200, json!({"id": id, "accepted": true}))
    }
}

impl Service for FeedHost {
    fn answer(&self, request: &Request<'_>) -> Response {
        let client = match self.gate.admit(request) {
            Ok(client) => client.name.clone(),
            Err(refused) => return http::echo(request, refused),
        };
        let parts: Vec<&str> = request.path.split('/').filter(|p| !p.is_empty()).collect();
        let answer = match (request.method, parts.as_slice()) {
            ("POST", ["feeds"]) => self.keyed(request, &client, |body| self.create(body)),
            ("GET", ["feeds", id]) => {
                let book = self.book.lock().expect("feeds lock");
                match book.feeds.get(*id) {
                    Some(feed) => Response::json(200, Self::metadata(id, feed)),
                    None => http::error(
                        404,
                        "invalid_request",
                        "feed_not_found",
                        "Feed not found",
                        Some("id"),
                    ),
                }
            }
            ("GET", ["feeds", id, "products"]) => {
                let book = self.book.lock().expect("feeds lock");
                match book.feeds.get(*id) {
                    Some(feed) => Response::json(
                        200,
                        json!({"products": feed.products.values().collect::<Vec<_>>()}),
                    ),
                    None => http::error(
                        404,
                        "invalid_request",
                        "feed_not_found",
                        "Feed not found",
                        Some("id"),
                    ),
                }
            }
            ("PATCH", ["feeds", id, "products"]) => {
                self.keyed(request, &client, |body| self.upsert(id, body))
            }
            _ => http::not_found("no such endpoint"),
        };
        http::echo(request, answer)
    }
}

impl FeedHost {
    /// Runs a mutating request under its idempotency key, if it has one
    /// (ACP says these SHOULD use one).
    fn keyed(
        &self,
        request: &Request<'_>,
        client: &str,
        then: impl FnOnce(&Value) -> Response,
    ) -> Response {
        let body = match http::body_of(request) {
            Ok(body) => body,
            Err(refused) => return refused,
        };
        let key = match http::key_of(request, false) {
            Ok(key) => key,
            Err(refused) => return refused,
        };
        let Some(key) = key else {
            return then(&body);
        };
        let slot = {
            let mut book = self.book.lock().expect("feeds lock");
            match http::claim(&mut book.keys, 422, client, request.path, key, &body) {
                Ok(slot) => slot,
                Err(answered) => return answered,
            }
        };
        let response = then(&body);
        let mut book = self.book.lock().expect("feeds lock");
        http::settle(&mut book.keys, slot, &response);
        let _ = self.save(&book);
        response
    }
}

// ----- The merchant's side ------------------------------------------------

/// A catalog as ACP feed products: one per offer, its one variant the
/// checkout item, priced in cents, sold by `seller` at `base_url`.
pub fn products_of(catalog: &Catalog, seller: &str, base_url: &str) -> Vec<Value> {
    catalog
        .offers
        .values()
        .map(|offer| {
            let cents = (offer.price.0 / super::MICROS_PER_CENT) as u64;
            let mut variant = json!({
                "id": offer.id,
                "title": offer.name,
                "price": {"amount": cents, "currency": "USD"},
                "availability": {"available": true, "status": "in_stock"},
                "seller": {"name": seller, "links": [{"type": "storefront", "url": base_url}]},
            });
            if !offer.description.is_empty() {
                variant["description"] = json!({"plain": offer.description});
            }
            let mut product = json!({
                "id": offer.id,
                "title": offer.name,
                "variants": [variant],
            });
            if !offer.description.is_empty() {
                product["description"] = json!({"plain": offer.description});
            }
            product
        })
        .collect()
}

/// Publishes `products` to the agent's Feed API at `feed_api`, into `feed`
/// (made, for `target_country`, if none is given): the feed's id.
pub fn publish(
    feed_api: &str,
    token: &str,
    feed: Option<&str>,
    target_country: Option<&str>,
    products: &[Value],
) -> Result<String, CliError> {
    let client = http::outbound(true)?;
    let base = feed_api.trim_end_matches('/');
    let bearer = format!("Bearer {token}");
    let send = |method: &str, path: &str, body: &Value| {
        let text = body.to_string();
        let key = http::random_hex(16);
        http::call(
            &client,
            method,
            &format!("{base}{path}"),
            &[
                ("Authorization", bearer.as_str()),
                ("API-Version", super::API_VERSION),
                ("Content-Type", "application/json"),
                ("Idempotency-Key", key.as_str()),
            ],
            Some(text.as_bytes()),
        )
    };
    let id = match feed {
        Some(id) => id.to_string(),
        None => {
            let mut request = json!({});
            if let Some(country) = target_country {
                request["target_country"] = json!(country);
            }
            let (status, made) = send("POST", "/feeds", &request)?;
            if status != 201 {
                return Err(CliError::Protocol(format!(
                    "the Feed API did not make a feed ({status}): {}",
                    made["message"]
                )));
            }
            made["id"].as_str().unwrap_or_default().to_string()
        }
    };
    let (status, answer) = send(
        "PATCH",
        &format!("/feeds/{id}/products"),
        &json!({"products": products}),
    )?;
    if status != 200 || answer["accepted"] != true {
        return Err(CliError::Protocol(format!(
            "the Feed API did not take the products ({status}): {}",
            answer["message"]
        )));
    }
    Ok(id)
}

/// Writes ACP's offline feed snapshot to `dir`: `metadata.json` and one
/// product per line in `products.jsonl`.
pub fn export(
    dir: &Path,
    feed: &str,
    target_country: Option<&str>,
    products: &[Value],
) -> Result<(), CliError> {
    let io = |e: std::io::Error| CliError::Usage(format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(io)?;
    let mut metadata = json!({"id": feed, "updated_at": http::rfc3339(http::now())});
    if let Some(country) = target_country {
        metadata["target_country"] = json!(country);
    }
    std::fs::write(dir.join("metadata.json"), format!("{metadata}\n")).map_err(io)?;
    let lines: String = products.iter().map(|p| format!("{p}\n")).collect();
    std::fs::write(dir.join("products.jsonl"), lines).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::from_json(&json!({"offers": [
            {"id": "report", "name": "A research report", "description": "cited", "price_usd": "4.50"},
        ]}))
        .unwrap()
    }

    #[test]
    fn a_catalog_becomes_products_whose_variant_is_the_checkout_item() {
        let products = products_of(&catalog(), "Research desk", "https://shop.example");
        assert_eq!(products.len(), 1);
        assert!(check_product(&products[0], "$").is_ok());
        let variant = &products[0]["variants"][0];
        assert_eq!(variant["id"], "report");
        assert_eq!(variant["price"], json!({"amount": 450, "currency": "USD"}));
    }

    #[test]
    fn a_product_without_variants_or_titles_is_refused() {
        assert!(check_product(&json!({"id": "p"}), "$").is_err());
        let untitled = json!({"id": "p", "variants": [{"id": "v"}]});
        assert_eq!(
            check_product(&untitled, "$").unwrap_err().1,
            "$.variants[0].title"
        );
        let bad_price = json!({"id": "p", "variants": [{"id": "v", "title": "t", "price": {"amount": 1.5, "currency": "USD"}}]});
        assert!(check_product(&bad_price, "$").is_err());
    }

    #[test]
    fn a_snapshot_is_metadata_and_one_product_a_line() {
        let dir = std::env::temp_dir().join(format!("sion-feed-{}", std::process::id()));
        let products = products_of(&catalog(), "Research desk", "https://shop.example");
        export(&dir, "feed_1", Some("US"), &products).unwrap();
        let metadata: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("metadata.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["id"], "feed_1");
        let lines = std::fs::read_to_string(dir.join("products.jsonl")).unwrap();
        assert_eq!(lines.lines().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
