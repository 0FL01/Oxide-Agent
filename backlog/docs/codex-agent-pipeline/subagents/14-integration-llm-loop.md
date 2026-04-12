# 14. Integration into Real LLM Loop

In a production framework, `run_turn()` typically does the following:

1. build prompt plus history;
2. call the LLM;
3. parse the response into one of:
   - final answer,
   - tool calls,
   - spawn or wait or send-input or close operations;
4. if a batch of tool calls is returned, execute it with `execute_batch`;
5. append tool outputs to history;
6. repeat until a final answer is reached.

In practice, `spawn_agent`, `wait_agent`, `send_input`, and `close_agent` should usually be implemented as ordinary tools backed by `AgentControl`.
