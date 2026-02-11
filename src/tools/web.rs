use async_trait::async_trait;
use tracing::debug;

// --- WebSearch ---

pub struct WebSearchTool {
    pub api_key: String,
    pub max_results: u32,
}

#[async_trait]
impl super::base::Tool for WebSearchTool {
    fn name(&self) -> &str { "web_search" }

    fn description(&self) -> &str {
        "Search the web using Brave Search API."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'query'".into()))?;

        if self.api_key.is_empty() {
            return Err(crate::error::NanobotError::Tool(
                "Web search API key not configured".into(),
            ));
        }

        debug!("web_search: {}", query);

        let client = reqwest::Client::new();
        let resp = client
            .get("https://api.search.brave.com/res/v1/web/search")
            .header("X-Subscription-Token", &self.api_key)
            .query(&[("q", query), ("count", &self.max_results.to_string())])
            .send()
            .await
            .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

        let results = data["web"]["results"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|r| {
                        format!(
                            "**{}**\n{}\n{}",
                            r["title"].as_str().unwrap_or(""),
                            r["url"].as_str().unwrap_or(""),
                            r["description"].as_str().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_else(|| "No results found.".to_string());

        Ok(results)
    }
}

// --- WebFetch ---

pub struct WebFetchTool;

#[async_trait]
impl super::base::Tool for WebFetchTool {
    fn name(&self) -> &str { "web_fetch" }

    fn description(&self) -> &str {
        "Fetch and extract readable content from a URL."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "URL to fetch" }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'url'".into()))?;

        debug!("web_fetch: {}", url);

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

        let resp = client
            .get(url)
            .header("User-Agent", "Nanobot/1.0")
            .send()
            .await
            .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

        let html = resp
            .text()
            .await
            .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

        // Basic HTML stripping — use scraper for readability extraction
        let doc = scraper::Html::parse_document(&html);
        let text: String = doc.root_element().text().collect::<Vec<_>>().join(" ");

        // Truncate to ~15KB
        let max = 15_000;
        if text.len() > max {
            Ok(format!("{}...\n[truncated]", &text[..max]))
        } else {
            Ok(text)
        }
    }
}
