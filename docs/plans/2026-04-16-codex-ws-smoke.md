# Codex CLI WebSocket Smoke Test

Verify the end-to-end Codex → lunaroute → OpenAI path using the WS transport.

1. Start lunaroute: `eval $(lunaroute-server env)`.
2. Edit `~/.codex/config.toml`:

   ```toml
   [model_providers.openai]
   name = "OpenAI"
   base_url = "http://127.0.0.1:8081/v1"
   env_key = "OPENAI_API_KEY"
   wire_api = "responses"
   supports_websockets = true
   ```

3. Run a trivial Codex command, e.g. `codex exec "print hello world in rust"`.
4. Open the lunaroute UI at `http://127.0.0.1:8082` — verify:
   - The session shows up.
   - Tokens are non-zero.
   - Response text matches what Codex displayed.
5. Optionally, to watch WS lifecycle logs, run the server in the foreground with debug logging instead of `eval $(lunaroute-server env)`:

   ```bash
   LUNAROUTE_LOG_LEVEL=debug lunaroute-server serve
   ```

   You should see `WS session started` then `WS session ended` for each request.
