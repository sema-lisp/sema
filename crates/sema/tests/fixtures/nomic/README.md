# Nomic text embedding fixture

`embed-response.json` is synthetic, not a recorded API response. Its fields and
types follow Nomic's published response schema, checked on 2026-10-20:

https://docs.nomic.ai/reference/api/embed-text-v-1-embedding-text-post

The required fields are `embeddings` (array of arrays of numbers), `usage`
(`prompt_tokens` and `total_tokens`, both integers), and `model` (string).
The short vectors make order and value assertions readable; they are not model
outputs. The tests request the same dimensionality as the fixture.

Request translation is also checked against that reference: bearer auth,
`POST /v1/embedding/text`, `texts`, `model`, `task_type`, `dimensionality`,
`long_text_mode`, and `max_tokens_per_text`. Nomic's official SDK corroborates
the endpoint, task names, and response layout:

https://github.com/nomic-ai/nomic/blob/b963f429ea40a40071257465253737db42ef6222/nomic/embed.py

These tests do not establish that the account has access or that the service
matches its documentation. The live smoke test is a separate release gate.
