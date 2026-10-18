use bytes::Bytes;
use http::uri::Authority;
use http::{HeaderMap, HeaderName, Method, Request, StatusCode, Uri};
use serde::Deserialize;

use crate::http::request;

mod deser_util;

use self::deser_util::{
    deserialize_base64, deserialize_header_names, deserialize_optional_authority,
    deserialize_optional_header_map, deserialize_optional_method, deserialize_status_code,
};

#[derive(Clone, Copy)]
pub(super) enum RuleOutcome {
    Continue,
    Deny(StatusCode),
}

#[derive(Clone)]
pub(super) struct RuleReport {
    pub(super) outcome: RuleOutcome,
    pub(super) matched_rules: Vec<String>,
}

#[derive(Default, Deserialize)]
pub(super) struct Rules {
    rules: Vec<Rule>,
}

impl Rules {
    pub(super) fn apply(&self, original_uri: &Uri, request: &mut Request<Bytes>) -> RuleReport {
        let mut matched_rules: Vec<String> = Vec::new();

        for rule in &self.rules {
            match rule.apply(original_uri, request) {
                Some(RuleOutcome::Continue) => {
                    matched_rules.push(rule.name.clone());
                }
                Some(RuleOutcome::Deny(code)) => {
                    matched_rules.push(rule.name.clone());

                    return RuleReport {
                        outcome: RuleOutcome::Deny(code),
                        matched_rules,
                    };
                }
                None => {}
            }
        }

        RuleReport {
            outcome: RuleOutcome::Continue,
            matched_rules,
        }
    }
}

#[derive(Deserialize)]
struct Rule {
    name: String,
    enabled: Option<bool>,
    #[serde(rename = "match")]
    r#match: RuleMatch,
    deny: Option<RuleDeny>,
    headers: Option<RuleHeaders>,
    body: Option<RuleBody>,
}

impl Rule {
    #[inline]
    fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn apply(&self, original_uri: &Uri, request: &mut Request<Bytes>) -> Option<RuleOutcome> {
        // Continue if the rule is disabled
        if !self.is_enabled() {
            return None;
        }

        // Continue if the rule does not match
        if !self.r#match.check(original_uri, request) {
            return None;
        }

        if let Some(deny) = &self.deny {
            return Some(RuleOutcome::Deny(deny.code));
        }

        // Apply the header rules
        if let Some(headers) = &self.headers {
            headers.apply(request.headers_mut());
        }

        // Apply the body rules
        if let Some(body) = &self.body {
            body.apply(request.body_mut());
        }

        // Normalize the body headers (i.e., Content-Length)
        request::normalize_body_headers(request);

        Some(RuleOutcome::Continue)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum RuleMatchPath {
    Full { path: String },
    Prefix { path_prefix: String },
}

#[derive(Default, Deserialize)]
struct RuleMatch {
    #[serde(default, deserialize_with = "deserialize_optional_authority")]
    authority: Option<Authority>,
    #[serde(default, deserialize_with = "deserialize_optional_method")]
    method: Option<Method>,
    #[serde(default, flatten)]
    path: Option<RuleMatchPath>,
}

impl RuleMatch {
    /// Returns `true` if a request matches the rule, otherwise `false`.
    fn check(&self, original_uri: &Uri, request: &Request<Bytes>) -> bool {
        if let Some(authority) = self.authority.as_ref() {
            if original_uri.authority() != Some(authority) {
                return false;
            }
        }

        if let Some(method) = self.method.as_ref() {
            if request.method() != method {
                return false;
            }
        }

        if let Some(path) = self.path.as_ref() {
            let found_path: &str = request.uri().path();

            match path {
                RuleMatchPath::Full { path } => {
                    if found_path != path {
                        return false;
                    }
                }
                RuleMatchPath::Prefix { path_prefix } => {
                    if !found_path.starts_with(path_prefix) {
                        return false;
                    }
                }
            }
        }

        true
    }
}

#[derive(Default, Deserialize)]
struct RuleDeny {
    #[serde(deserialize_with = "deserialize_status_code")]
    code: StatusCode,
}

#[derive(Default, Deserialize)]
struct RuleHeaders {
    #[serde(default, deserialize_with = "deserialize_optional_header_map")]
    set: Option<HeaderMap>,
    #[serde(default, deserialize_with = "deserialize_header_names")]
    remove: Vec<HeaderName>,
}

impl RuleHeaders {
    fn apply(&self, headers: &mut HeaderMap) {
        if let Some(set) = &self.set {
            headers.extend(set.clone());
        }

        for header_name in &self.remove {
            headers.remove(header_name);
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum RuleBodySet {
    Utf8 {
        set_utf8: String,
    },
    Base64 {
        #[serde(deserialize_with = "deserialize_base64")]
        set_base64: Vec<u8>,
    },
}

#[derive(Deserialize)]
struct RuleBodyReplacement {
    from: String,
    to: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RuleBody {
    // Completely replace the body
    Set {
        #[serde(flatten)]
        set: RuleBodySet,
    },
    Replace {
        replace: Vec<RuleBodyReplacement>,
    },
}

impl RuleBody {
    fn apply(&self, body: &mut Bytes) {
        match self {
            Self::Set { set } => match set {
                RuleBodySet::Utf8 { set_utf8 } => {
                    *body = Bytes::copy_from_slice(set_utf8.as_bytes());
                }
                RuleBodySet::Base64 { set_base64 } => {
                    *body = Bytes::copy_from_slice(set_base64);
                }
            },
            Self::Replace { replace } => {
                let mut body_str: String = String::from_utf8_lossy(body).to_string();

                for replacement in replace {
                    body_str = body_str.replace(&replacement.from, &replacement.to);
                }

                *body = Bytes::copy_from_slice(body_str.as_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &'static str = include_str!("example.toml");

    #[test]
    fn deserialize_example() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        assert_eq!(rules.rules.len(), 4);
    }

    #[test]
    fn rule_match() {
        let uri = Uri::from_static("http://example.com/api/profile");
        let path = Uri::from_static("/api/profile");
        let request: Request<Bytes> = Request::builder()
            .uri(path)
            .method(Method::GET)
            .body(Bytes::new())
            .unwrap();

        // Must always match the default rule
        let r = RuleMatch::default();
        assert!(r.check(&uri, &request));

        // Set authority
        let mut r = RuleMatch::default();
        r.authority = Some(Authority::from_static("example.com"));
        assert!(r.check(&uri, &request));
        r.authority = Some(Authority::from_static("example.org"));
        assert!(!r.check(&uri, &request));

        // Match the authority port
        let port_uri = Uri::from_static("http://example.com:8000/api/profile");
        r.authority = Some(Authority::from_static("example.com:8000"));
        assert!(r.check(&port_uri, &request));
        r.authority = Some(Authority::from_static("example.com:8001"));
        assert!(!r.check(&port_uri, &request));

        // An omitted port differs from an explicit default port
        r.authority = Some(Authority::from_static("example.com:80"));
        assert!(!r.check(&uri, &request));

        // Set method
        let mut r = RuleMatch::default();
        r.method = Some(Method::GET);
        assert!(r.check(&uri, &request));
        r.method = Some(Method::POST);
        assert!(!r.check(&uri, &request));

        // Set path
        let mut r = RuleMatch::default();
        r.path = Some(RuleMatchPath::Full {
            path: String::from("/api/profile"),
        });
        assert!(r.check(&uri, &request));
        r.path = Some(RuleMatchPath::Full {
            path: String::from("/api/other"),
        });
        assert!(!r.check(&uri, &request));

        // Change path prefix
        let mut r = RuleMatch::default();
        r.path = Some(RuleMatchPath::Prefix {
            path_prefix: String::from("/api"),
        });
        assert!(r.check(&uri, &request));
        r.path = Some(RuleMatchPath::Prefix {
            path_prefix: String::from("/login"),
        });
        assert!(!r.check(&uri, &request));
    }

    #[test]
    fn rule_headers_apply() {
        let mut headers = HeaderMap::new();
        headers.insert("x-keep", "keep".parse().unwrap());
        headers.insert("x-update", "old".parse().unwrap());
        headers.insert("x-remove", "remove".parse().unwrap());

        let mut set = HeaderMap::new();
        set.insert("x-add", "new".parse().unwrap());
        set.insert("x-update", "updated".parse().unwrap());

        let mut r = RuleHeaders {
            set: Some(set),
            remove: vec![
                HeaderName::from_static("x-remove"),
                HeaderName::from_static("x-missing"),
            ],
        };

        // Add, overwrite, and remove headers
        r.apply(&mut headers);
        assert_eq!(headers.get("x-keep").unwrap(), "keep");
        assert_eq!(headers.get("x-add").unwrap(), "new");
        assert_eq!(headers.get("x-update").unwrap(), "updated");
        assert!(!headers.contains_key("x-remove"));
        assert!(!headers.contains_key("x-missing"));
        assert_eq!(headers.len(), 3);

        // Remove takes precedence over set
        r.remove.push(HeaderName::from_static("x-add"));
        r.apply(&mut headers);
        assert!(!headers.contains_key("x-add"));
        assert_eq!(headers.get("x-update").unwrap(), "updated");
        assert_eq!(headers.len(), 2);
    }

    #[test]
    fn rule_body_apply() {
        let mut body: Bytes = Bytes::from_static(b"original");

        // Replace the entire body with UTF-8 text
        let r = RuleBody::Set {
            set: RuleBodySet::Utf8 {
                set_utf8: String::from("modified"),
            },
        };
        r.apply(&mut body);
        assert_eq!(body.as_ref(), "modified".as_bytes());

        // Replace the entire body with base64 bytes
        let r = RuleBody::Set {
            set: RuleBodySet::Base64 {
                set_base64: vec![0, 255, 128],
            },
        };
        r.apply(&mut body);
        assert_eq!(body.as_ref(), &[0, 255, 128]);

        // Apply all literal replacements in order
        body = Bytes::from_static(b"Hello, world!");
        let r = RuleBody::Replace {
            replace: vec![
                RuleBodyReplacement {
                    from: String::from("Hello"),
                    to: String::from("Hola"),
                },
                RuleBodyReplacement {
                    from: String::from("world"),
                    to: String::from("mundo"),
                },
            ],
        };
        r.apply(&mut body);
        assert_eq!(body.as_ref(), b"Hola, mundo!");

        // Leave text without matches unchanged
        body = Bytes::from_static(b"unchanged");
        r.apply(&mut body);
        assert_eq!(body.as_ref(), b"unchanged");
    }

    #[test]
    fn rule_apply() {
        let uri = Uri::from_static("http://example.com/api/profile");
        let mut request: Request<Bytes> = Request::builder()
            .uri("/api/profile")
            .method(Method::POST)
            .header("content-length", "3")
            .header("x-state", "original")
            .body(Bytes::from_static(b"old"))
            .unwrap();
        let original = request::format_request(&request).unwrap();

        let mut set = HeaderMap::new();
        set.insert("x-state", "updated".parse().unwrap());

        let mut r = Rule {
            name: String::from("edit-profile"),
            enabled: Some(false),
            r#match: RuleMatch::default(),
            deny: None,
            headers: Some(RuleHeaders {
                set: Some(set),
                remove: Vec::new(),
            }),
            body: Some(RuleBody::Set {
                set: RuleBodySet::Utf8 {
                    set_utf8: String::from("updated body"),
                },
            }),
        };

        // Skip disabled rules
        assert!(r.apply(&uri, &mut request).is_none());
        assert_eq!(request::format_request(&request).unwrap(), original);

        // Skip rules that do not match
        r.enabled = None;
        r.r#match.authority = Some(Authority::from_static("example.org"));
        assert!(r.apply(&uri, &mut request).is_none());
        assert_eq!(request::format_request(&request).unwrap(), original);

        // Deny before applying header or body changes
        r.r#match.authority = Some(Authority::from_static("example.com"));
        r.deny = Some(RuleDeny {
            code: StatusCode::TOO_MANY_REQUESTS,
        });
        assert!(matches!(
            r.apply(&uri, &mut request),
            Some(RuleOutcome::Deny(StatusCode::TOO_MANY_REQUESTS))
        ));
        assert_eq!(request::format_request(&request).unwrap(), original);

        // Rules are enabled by default and normalize Content-Length
        r.deny = None;
        assert!(matches!(
            r.apply(&uri, &mut request),
            Some(RuleOutcome::Continue)
        ));
        assert_eq!(request.headers().get("x-state").unwrap(), "updated");
        assert_eq!(request.body().as_ref(), b"updated body");
        assert_eq!(request.headers().get("content-length").unwrap(), "12");
        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri(), "/api/profile");
    }

    #[test]
    fn rules_apply() {
        let uri = Uri::from_static("http://example.com/api/profile");
        let mut request: Request<Bytes> = Request::builder()
            .uri("/api/profile")
            .method(Method::POST)
            .body(Bytes::from_static(b"original"))
            .unwrap();
        let original = request::format_request(&request).unwrap();
        let mut rules = Rules::default();

        // An empty rule list leaves the request unchanged
        let report = rules.apply(&uri, &mut request);
        assert!(matches!(report.outcome, RuleOutcome::Continue));
        assert!(report.matched_rules.is_empty());
        assert_eq!(request::format_request(&request).unwrap(), original);

        // Skipped rules are not included in the report
        rules.rules.push(Rule {
            name: String::from("disabled"),
            enabled: Some(false),
            r#match: RuleMatch::default(),
            deny: Some(RuleDeny {
                code: StatusCode::FORBIDDEN,
            }),
            headers: None,
            body: None,
        });
        rules.rules.push(Rule {
            name: String::from("other-authority"),
            enabled: None,
            r#match: RuleMatch {
                authority: Some(Authority::from_static("example.org")),
                ..RuleMatch::default()
            },
            deny: Some(RuleDeny {
                code: StatusCode::FORBIDDEN,
            }),
            headers: None,
            body: None,
        });
        let report = rules.apply(&uri, &mut request);
        assert!(matches!(report.outcome, RuleOutcome::Continue));
        assert!(report.matched_rules.is_empty());
        assert_eq!(request::format_request(&request).unwrap(), original);

        // Body changes accumulate in declaration order
        rules.rules.push(Rule {
            name: String::from("set-body"),
            enabled: None,
            r#match: RuleMatch::default(),
            deny: None,
            headers: None,
            body: Some(RuleBody::Set {
                set: RuleBodySet::Utf8 {
                    set_utf8: String::from("first"),
                },
            }),
        });
        rules.rules.push(Rule {
            name: String::from("replace-body"),
            enabled: Some(true),
            r#match: RuleMatch::default(),
            deny: None,
            headers: None,
            body: Some(RuleBody::Replace {
                replace: vec![RuleBodyReplacement {
                    from: String::from("first"),
                    to: String::from("second"),
                }],
            }),
        });
        let report = rules.apply(&uri, &mut request);
        assert!(matches!(report.outcome, RuleOutcome::Continue));
        assert_eq!(report.matched_rules, vec!["set-body", "replace-body"]);
        assert_eq!(request.body().as_ref(), b"second");
        assert_eq!(request.headers().get("content-length").unwrap(), "6");

        // Matching rules are reported even when no bytes change
        rules.rules.push(Rule {
            name: String::from("match-only"),
            enabled: None,
            r#match: RuleMatch::default(),
            deny: None,
            headers: None,
            body: None,
        });
        let before = request::format_request(&request).unwrap();
        let report = rules.apply(&uri, &mut request);
        assert!(matches!(report.outcome, RuleOutcome::Continue));
        assert_eq!(
            report.matched_rules,
            vec!["set-body", "replace-body", "match-only"]
        );
        assert_eq!(request::format_request(&request).unwrap(), before);

        // Deny preserves earlier changes and stops later rules
        rules.rules.push(Rule {
            name: String::from("block-profile"),
            enabled: None,
            r#match: RuleMatch::default(),
            deny: Some(RuleDeny {
                code: StatusCode::FORBIDDEN,
            }),
            headers: None,
            body: None,
        });
        rules.rules.push(Rule {
            name: String::from("after-deny"),
            enabled: None,
            r#match: RuleMatch::default(),
            deny: None,
            headers: None,
            body: Some(RuleBody::Set {
                set: RuleBodySet::Utf8 {
                    set_utf8: String::from("must not be applied"),
                },
            }),
        });
        *request.body_mut() = Bytes::from_static(b"original");
        let report = rules.apply(&uri, &mut request);
        assert!(matches!(
            report.outcome,
            RuleOutcome::Deny(StatusCode::FORBIDDEN)
        ));
        assert_eq!(
            report.matched_rules,
            vec!["set-body", "replace-body", "match-only", "block-profile"]
        );
        assert_eq!(request.body().as_ref(), b"second");
        assert_eq!(request.headers().get("content-length").unwrap(), "6");
    }
}
