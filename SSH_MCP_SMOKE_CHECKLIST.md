# SSH MCP Provider - Smoke Testing Checklist

After deploying the new upstream `ssh-mcp`-backed provider, run these commands in a Telegram AgentMode topic to verify the fix.

## Basic Command Execution
- [ ] Run `echo test` - should return immediately without timeout
- [ ] Run `echo test` again (repeated) - should not degrade or timeout
- [ ] Run `date` - should return current timestamp
- [ ] Run `hostname` - should return hostname
- [ ] Run `id` - should return user info

## System Inspection Commands (previously problematic)
- [ ] Run `ls -la /tmp` - should list directory contents
- [ ] Run `ls -lh /var/log/` - should show log directory
- [ ] Run `ps aux | head -10` - should show process list
- [ ] Run `journalctl -n 100` - should show recent journal entries
- [ ] Run `ss -tn` - should show network connections
- [ ] Run `uptime` - should show system uptime

## Command Sequence Testing
- [ ] Run 10 sequential commands in a row without pause
  - echo "test 1"
  - echo "test 2"
  - ...
  - echo "test 10"
- [ ] Verify all 10 commands completed successfully

## Error Handling Verification
- [ ] Run an invalid command (e.g., `nonexistent_cmd`) - should fail gracefully
- [ ] Run a command that times out (if any) - should timeout cleanly

## Background/Long-running Commands (if available)
- [ ] Run `sleep 5` - should handle timeout or background mode
- [ ] Check process status after timeout - verify cleanup

## Key Success Indicators
- [ ] No consistent 30-second timeout on simple commands
- [ ] No degradation after multiple sequential commands
- [ ] Zombie processes cleaned up on timeout/cancellation
- [ ] Approval flow still works as expected
- [ ] All tool modes (exec, sudo-exec, read-file, apply-file-edit, check-process) functional

## Notes
Fill in observations during testing:

- Commands that worked:
- Commands that failed/timed out:
- Overall success rate:
- Observed improvements over old implementation:
