# Tham khảo lệnh RustyClaw

Dựa trên CLI hiện tại (`rustyclaw --help`).

Xác minh lần cuối: **2026-02-20**.

## Lệnh cấp cao nhất

| Lệnh | Mục đích |
|---|---|
| `onboard` | Khởi tạo workspace/config nhanh hoặc tương tác |
| `agent` | Chạy chat tương tác hoặc chế độ gửi tin nhắn đơn |
| `gateway` | Khởi động gateway webhook và HTTP WhatsApp |
| `daemon` | Khởi động runtime có giám sát (gateway + channels + heartbeat/scheduler tùy chọn) |
| `service` | Quản lý vòng đời dịch vụ cấp hệ điều hành |
| `doctor` | Chạy chẩn đoán và kiểm tra trạng thái |
| `status` | Hiển thị cấu hình và tóm tắt hệ thống |
| `cron` | Quản lý tác vụ định kỳ |
| `models` | Làm mới danh mục model của provider |
| `providers` | Liệt kê ID provider, bí danh và provider đang dùng |
| `channel` | Quản lý kênh và kiểm tra sức khỏe kênh |
| `integrations` | Kiểm tra chi tiết tích hợp |
| `skills` | Liệt kê/cài đặt/gỡ bỏ skills |
| `migrate` | Nhập dữ liệu từ runtime khác (hiện hỗ trợ OpenClaw) |
| `config` | Xuất schema cấu hình dạng máy đọc được |
| `completions` | Tạo script tự hoàn thành cho shell ra stdout |
| `hardware` | Phát hiện và kiểm tra phần cứng USB |
| `peripheral` | Cấu hình và nạp firmware thiết bị ngoại vi |

## Nhóm lệnh

### `onboard`

- `rustyclaw onboard`
- `rustyclaw onboard --interactive`
- `rustyclaw onboard --channels-only`
- `rustyclaw onboard --api-key <KEY> --provider <ID> --memory <sqlite|lucid|markdown|none>`
- `rustyclaw onboard --api-key <KEY> --provider <ID> --model <MODEL_ID> --memory <sqlite|lucid|markdown|none>`

### `agent`

- `rustyclaw agent`
- `rustyclaw agent -m "Hello"`
- `rustyclaw agent --provider <ID> --model <MODEL> --temperature <0.0-2.0>`
- `rustyclaw agent --peripheral <board:path>`

### `gateway` / `daemon`

- `rustyclaw gateway [--host <HOST>] [--port <PORT>]`
- `rustyclaw daemon [--host <HOST>] [--port <PORT>]`

### `service`

- `rustyclaw service install`
- `rustyclaw service start`
- `rustyclaw service stop`
- `rustyclaw service restart`
- `rustyclaw service status`
- `rustyclaw service uninstall`

### `cron`

- `rustyclaw cron list`
- `rustyclaw cron add <expr> [--tz <IANA_TZ>] <command>`
- `rustyclaw cron add-at <rfc3339_timestamp> <command>`
- `rustyclaw cron add-every <every_ms> <command>`
- `rustyclaw cron once <delay> <command>`
- `rustyclaw cron remove <id>`
- `rustyclaw cron pause <id>`
- `rustyclaw cron resume <id>`

### `models`

- `rustyclaw models refresh`
- `rustyclaw models refresh --provider <ID>`
- `rustyclaw models refresh --force`

`models refresh` hiện hỗ trợ làm mới danh mục trực tiếp cho các provider: `openrouter`, `openai`, `anthropic`, `groq`, `mistral`, `deepseek`, `xai`, `together-ai`, `gemini`, `ollama`, `astrai`, `venice`, `fireworks`, `cohere`, `moonshot`, `glm`, `zai`, `qwen` và `nvidia`.

### `channel`

- `rustyclaw channel list`
- `rustyclaw channel start`
- `rustyclaw channel doctor`
- `rustyclaw channel bind-telegram <IDENTITY>`
- `rustyclaw channel add <type> <json>`
- `rustyclaw channel remove <name>`

Lệnh trong chat khi runtime đang chạy (Telegram/Discord):

- `/models`
- `/models <provider>`
- `/model`
- `/model <model-id>`

Channel runtime cũng theo dõi `config.toml` và tự động áp dụng thay đổi cho:
- `default_provider`
- `default_model`
- `default_temperature`
- `api_key` / `api_url` (cho provider mặc định)
- `reliability.*` cài đặt retry của provider

`add/remove` hiện chuyển hướng về thiết lập có hướng dẫn / cấu hình thủ công (chưa hỗ trợ đầy đủ mutator khai báo).

### `integrations`

- `rustyclaw integrations info <name>`

### `skills`

- `rustyclaw skills list`
- `rustyclaw skills install <source>`
- `rustyclaw skills remove <name>`

`<source>` chấp nhận git remote (`https://...`, `http://...`, `ssh://...` và `git@host:owner/repo.git`) hoặc đường dẫn cục bộ.

Skill manifest (`SKILL.toml`) hỗ trợ `prompts` và `[[tools]]`; cả hai được đưa vào system prompt của agent khi chạy, giúp model có thể tuân theo hướng dẫn skill mà không cần đọc thủ công.

### `migrate`

- `rustyclaw migrate openclaw [--source <path>] [--dry-run]`

### `config`

- `rustyclaw config schema`

`config schema` xuất JSON Schema (draft 2020-12) cho toàn bộ hợp đồng `config.toml` ra stdout.

### `completions`

- `rustyclaw completions bash`
- `rustyclaw completions fish`
- `rustyclaw completions zsh`
- `rustyclaw completions powershell`
- `rustyclaw completions elvish`

`completions` chỉ xuất ra stdout để script có thể được source trực tiếp mà không bị lẫn log/cảnh báo.

### `hardware`

- `rustyclaw hardware discover`
- `rustyclaw hardware introspect <path>`
- `rustyclaw hardware info [--chip <chip_name>]`

### `peripheral`

- `rustyclaw peripheral list`
- `rustyclaw peripheral add <board> <path>`
- `rustyclaw peripheral flash [--port <serial_port>]`
- `rustyclaw peripheral setup-uno-q [--host <ip_or_host>]`
- `rustyclaw peripheral flash-nucleo`

## Kiểm tra nhanh

Để xác minh nhanh tài liệu với binary hiện tại:

```bash
rustyclaw --help
rustyclaw <command> --help
```
