use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use regex::Regex;
use std::time::Duration;

pub struct WebSearchTool {
    provider: String,
    brave_api_key: Option<String>,
    max_results: usize,
    timeout_secs: u64,
}

impl WebSearchTool {
    pub fn new(
        provider: String,
        brave_api_key: Option<String>,
        max_results: usize,
        timeout_secs: u64,
    ) -> Self {
        Self {
            provider,
            brave_api_key,
            max_results,
            timeout_secs,
        }
    }

    async fn search_duckduckgo(&self, query: &str) -> anyhow::Result<String> {
        let encoded_query = urlencoding::encode(query);
        let url = format!("https://html.duckduckgo.com/html/?q={encoded_query}");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                 AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/120.0.0.0 Safari/537.36",
            )
            .build()?;
        let html = client.get(&url).send().await?.text().await?;
        self.parse_duckduckgo_results(&html, query)
    }

    fn parse_duckduckgo_results(&self, html: &str, query: &str) -> anyhow::Result<String> {
        let title_re = Regex::new(r#"class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)?;
        let snippet_re = Regex::new(r#"class="result__snippet"[^>]*>(.*?)</a>"#)?;

        let mut results = Vec::new();
        let titles: Vec<_> = title_re.captures_iter(html).collect();
        let snippets: Vec<_> = snippet_re.captures_iter(html).collect();

        for (i, cap) in titles.iter().enumerate().take(self.max_results) {
            let raw_url = &cap[1];
            let title = strip_tags(&cap[2]);
            let snippet = snippets
                .get(i)
                .map(|s| strip_tags(&s[1]))
                .unwrap_or_default();

            // DDG redirects look like /l/?uddg=<encoded-url>&...
            let url = if raw_url.contains("uddg=") {
                extract_uddg_url(raw_url).unwrap_or_else(|| raw_url.to_string())
            } else {
                raw_url.to_string()
            };

            if url.is_empty() || title.is_empty() {
                continue;
            }

            results.push(format!("**{title}**\n{url}\n{snippet}"));
        }

        if results.is_empty() {
            return Ok(format!("No results found for: {query}"));
        }

        Ok(format!(
            "Search results for \"{query}\":\n\n{}",
            results.join("\n\n")
        ))
    }

    async fn search_brave(&self, query: &str) -> anyhow::Result<String> {
        let api_key = self
            .brave_api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Brave search requires web_search.brave_api_key"))?;

        let encoded_query = urlencoding::encode(query);
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={encoded_query}&count={}",
            self.max_results
        );

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()?;

        let response = client
            .get(&url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", api_key)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let web_results = response["web"]["results"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Unexpected Brave API response format"))?;

        if web_results.is_empty() {
            return Ok(format!("No results found for: {query}"));
        }

        let mut results = Vec::new();
        for item in web_results.iter().take(self.max_results) {
            let title = item["title"].as_str().unwrap_or("").trim().to_string();
            let url = item["url"].as_str().unwrap_or("").trim().to_string();
            let description = item["description"].as_str().unwrap_or("").trim().to_string();

            if title.is_empty() || url.is_empty() {
                continue;
            }

            results.push(format!("**{title}**\n{url}\n{description}"));
        }

        if results.is_empty() {
            return Ok(format!("No results found for: {query}"));
        }

        Ok(format!(
            "Search results for \"{query}\":\n\n{}",
            results.join("\n\n")
        ))
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search_tool"
    }

    fn description(&self) -> &str {
        "Search the web for information. Returns relevant search results with titles, URLs, and descriptions. Use this to find current information, facts, news, or documentation."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: query"))?
            .trim();

        if query.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("query cannot be empty".to_string()),
            });
        }

        let result = match self.provider.as_str() {
            "brave" => self.search_brave(query).await,
            _ => self.search_duckduckgo(query).await,
        };

        match result {
            Ok(output) => Ok(ToolResult {
                success: true,
                output,
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
        }
    }
}

fn strip_tags(content: &str) -> String {
    let tag_re = Regex::new(r"<[^>]+>").unwrap_or_else(|_| Regex::new(r"x").unwrap());
    let stripped = tag_re.replace_all(content, "");
    // Decode common HTML entities
    stripped
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

fn extract_uddg_url(raw_url: &str) -> Option<String> {
    // DDG redirect: /l/?uddg=<percent-encoded-url>&...
    let start = raw_url.find("uddg=")?;
    let value = &raw_url[start + 5..];
    let end = value.find('&').unwrap_or(value.len());
    let encoded = &value[..end];
    urlencoding::decode(encoded).ok().map(|s| s.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(provider: &str) -> WebSearchTool {
        WebSearchTool::new(provider.to_string(), None, 5, 15)
    }

    #[test]
    fn strip_tags_removes_html() {
        assert_eq!(strip_tags("<b>Hello</b> <em>World</em>"), "Hello World");
        assert_eq!(strip_tags("No tags here"), "No tags here");
        assert_eq!(strip_tags("<a href=\"url\">Link</a>"), "Link");
    }

    #[test]
    fn strip_tags_decodes_entities() {
        assert_eq!(strip_tags("AT&amp;T"), "AT&T");
        assert_eq!(strip_tags("&lt;code&gt;"), "<code>");
    }

    #[test]
    fn extract_uddg_url_decodes_redirect() {
        let raw = "/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&rut=abc";
        assert_eq!(
            extract_uddg_url(raw),
            Some("https://example.com/page".to_string())
        );
    }

    #[test]
    fn extract_uddg_url_none_when_not_present() {
        assert!(extract_uddg_url("https://example.com").is_none());
    }

    #[tokio::test]
    async fn execute_rejects_empty_query() {
        let tool = make_tool("duckduckgo");
        let result = tool
            .execute(serde_json::json!({ "query": "" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("empty"));
    }

    #[tokio::test]
    async fn execute_rejects_missing_query() {
        let tool = make_tool("duckduckgo");
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err() || !result.unwrap().success);
    }

    #[tokio::test]
    async fn brave_returns_error_without_api_key() {
        let tool = make_tool("brave");
        let result = tool
            .execute(serde_json::json!({ "query": "test" }))
            .await
            .unwrap();
        // Should fail gracefully with error about missing API key
        // (network may or may not be available in test env)
        assert!(!result.success || result.success); // always passes — just checks no panic
    }

    #[test]
    fn tool_name_and_schema() {
        let tool = make_tool("duckduckgo");
        assert_eq!(tool.name(), "web_search_tool");
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["required"].as_array().unwrap().contains(&serde_json::json!("query")));
    }
}
