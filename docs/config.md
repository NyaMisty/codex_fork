# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).

## Automatic continuation fallback

After repeated interrupted responses that are recovered automatically with
`继续`, Codex can temporarily switch to another model:

```toml
[auto_continue]
max_retries = 3
fallback_model = "gpt-6.1-terra"
fallback_reasoning_effort = "high"
```

- `max_retries`: number of consecutive automatic-continuation failures allowed
  before switching. Defaults to 3.
- `fallback_model`: model used for the next attempt. Unset disables switching.
- `fallback_reasoning_effort`: reasoning effort for the fallback model. Unset
  uses the fallback model's default effort.

Any successful response resets the consecutive-failure count and restores the
original model. This fallback only applies within the current turn.

## Lifecycle hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.
