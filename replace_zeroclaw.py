import os

root = '/Users/sujshe/projects/rustyclaw'
files = [
    'tests/memory_comparison.rs', 'tests/memory_restart.rs',
    'tests/openai_codex_vision_e2e.rs', 'tests/provider_resolution.rs',
    'tests/provider_schema.rs', 'tests/telegram_attachment_fallback.rs',
    'tests/whatsapp_webhook_security.rs', 'src/util.rs',
    'tests/agent_e2e.rs', 'tests/agent_loop_robustness.rs',
    'tests/channel_routing.rs', 'tests/config_persistence.rs',
    'tests/config_schema.rs', 'tests/dockerignore_test.rs',
    'tests/gemini_fallback_oauth_refresh.rs', 'tests/hooks_integration.rs',
    'src/tools/shell.rs', 'src/tools/web_fetch.rs',
    'src/tools/image_info.rs', 'src/tools/mod.rs',
    'src/tools/proxy_config.rs', 'src/tools/schema.rs',
    'src/tools/file_edit.rs', 'src/tools/file_read.rs',
    'src/tools/file_write.rs', 'src/sop/mod.rs',
    'src/tools/browser.rs', 'src/tools/composio.rs',
    'src/service/mod.rs', 'src/skillforge/integrate.rs',
    'src/skillforge/mod.rs', 'src/skillforge/scout.rs',
    'src/skills/mod.rs', 'src/security/audit.rs',
    'src/security/mod.rs', 'src/security/otp.rs',
    'src/security/policy.rs', 'src/security/secrets.rs',
    'src/providers/openrouter.rs', 'src/providers/telnyx.rs',
    'src/runtime/docker.rs', 'src/runtime/native.rs',
    'src/runtime/wasm.rs', 'src/providers/anthropic.rs',
    'src/providers/compatible.rs', 'src/providers/copilot.rs',
    'src/providers/gemini.rs', 'src/providers/glm.rs',
    'src/providers/mod.rs', 'src/providers/openai_codex.rs',
    'src/providers/openai.rs', 'src/onboard/wizard.rs',
    'src/peripherals/arduino_flash.rs', 'src/peripherals/arduino_upload.rs',
    'src/peripherals/mod.rs', 'src/peripherals/nucleo_flash.rs',
    'src/peripherals/uno_q_bridge.rs', 'src/peripherals/uno_q_setup.rs',
    'src/observability/otel.rs', 'src/observability/prometheus.rs',
    'src/memory/lucid.rs', 'src/memory/postgres.rs',
    'src/memory/response_cache.rs', 'src/memory/snapshot.rs',
    'src/migration.rs', 'src/identity.rs',
    'src/integrations/mod.rs', 'src/lib.rs', 'src/main.rs',
    'src/memory/cli.rs', 'src/hardware/mod.rs',
    'src/heartbeat/engine.rs', 'src/cost/tracker.rs',
    'src/cron/mod.rs', 'src/daemon/mod.rs',
    'src/doctor/mod.rs', 'src/gateway/mod.rs',
    'src/channels/telegram.rs', 'src/channels/wati.rs',
    'src/channels/whatsapp_storage.rs', 'src/channels/whatsapp_web.rs',
    'src/channels/whatsapp.rs', 'src/config/schema.rs',
    'src/channels/linq.rs', 'src/channels/matrix.rs',
    'src/channels/mod.rs', 'src/channels/mqtt.rs',
    'src/channels/nextcloud_talk.rs', 'src/channels/qq.rs',
    'src/auth/openai_oauth.rs', 'src/channels/dingtalk.rs',
    'src/channels/discord.rs', 'src/channels/email_channel.rs',
    'src/channels/irc.rs', 'src/channels/lark.rs',
    'src/agent/agent.rs', 'src/agent/loop_.rs', 'src/agent/prompt.rs',
    'fuzz/fuzz_targets/fuzz_command_validation.rs',
    'firmware/rustyclaw-esp32/src/main.rs',
    'firmware/rustyclaw-nucleo/src/main.rs',
    'examples/custom_tool.rs',
    'firmware/rustyclaw-esp32-ui/src/main.rs',
    'examples/custom_channel.rs', 'examples/custom_memory.rs',
    'examples/custom_provider.rs',
    'crates/robot-kit/src/emote.rs', 'crates/robot-kit/src/lib.rs',
    'crates/robot-kit/src/listen.rs', 'crates/robot-kit/src/look.rs',
    'crates/robot-kit/src/speak.rs', 'crates/robot-kit/src/traits.rs',
    'benches/agent_benchmarks.rs',
]

replacements = [
    ('zeroclaw_orchestrator', 'rustyclaw_orchestrator'),
    ('ZeroclawOrchestrator', 'RustyclawOrchestrator'),
    ('ZEROCLAW', 'RUSTYCLAW'),
    ('ZeroClaw', 'RustyClaw'),
    ('Zeroclaw', 'Rustyclaw'),
    ('zeroclaw', 'rustyclaw'),
]

changed = 0
for f in files:
    path = os.path.join(root, f)
    if not os.path.exists(path):
        print('SKIP: ' + f)
        continue
    fh = open(path, 'r')
    content = fh.read()
    fh.close()
    original = content
    for old, new in replacements:
        content = content.replace(old, new)
    if content != original:
        fh = open(path, 'w')
        fh.write(content)
        fh.close()
        changed += 1
print('Changed ' + str(changed) + ' files out of ' + str(len(files)))
