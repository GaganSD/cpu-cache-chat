# Agent player

Container-local Pi integration. The process reads one JSON object on stdin:

```json
{"board":"123456789","turn":"X","legal":[1,2,3,4,5,6,7,8,9]}
```

and writes `{"square":5,"source":"pi"}`.

Only `get_board` and `submit_move` are exposed. Rust still validates the square.

`CCC_ALLOW_FIRST_LEGAL=1` is a test hook, not the demo.
