#!/bin/bash
# setup.sh - Setup script for Opencode + Sandbox integration
#
# This script helps set up the integration by:
# 1. Creating the architect agent
# 2. Starting Opencode server
# 3. Running health checks
# 4. Providing example usage

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
OPENCODE_PORT="${OPENCODE_PORT:-4096}"
OPENCODE_HOST="${OPENCODE_HOST:-127.0.0.1}"
AGENT_DIR="${AGENT_DIR:-.opencode/agent}"

echo "=========================================="
echo "Opencode + Sandbox Integration Setup"
echo "=========================================="
echo ""

# Step 1: Check if opencode is installed
echo -e "${YELLOW}[1/5]${NC} Checking if opencode is installed..."
if command -v opencode &> /dev/null; then
    echo -e "${GREEN}✓${NC} opencode is installed: $(opencode --version 2>/dev/null || echo 'version unknown')"
else
    echo -e "${RED}✗${NC} opencode is not installed"
    echo "Please install opencode first: https://opencode.ai/docs/installation"
    exit 1
fi
echo ""

# Step 2: Create architect agent
echo -e "${YELLOW}[2/5]${NC} Creating architect agent..."
mkdir -p "$AGENT_DIR"

cat > "$AGENT_DIR/architect.md" << 'EOF'
---
description: Orchestrates complex multi-step development tasks by delegating to specialized subagents
mode: primary
model: openai/gpt-4.1
color: "#9b59b6"
temperature: 0.3
permission:
  task:
    "*": "allow"
  edit: "allow"
  bash: "allow"
  read: "allow"
  glob: "allow"
  grep: "allow"
  webfetch: "allow"
  websearch: "allow"
  codesearch: "allow"
  external_directory:
    "*": "ask"
---

You are an architect agent responsible for orchestrating complex multi-step development tasks.

Your role is to:
1. Analyze the user's request and break it down into smaller, manageable tasks
2. Delegate tasks to specialized subagents using the Task tool (@explore for code exploration, @general for multi-step work, @developer for implementation, @review for code review)
3. Coordinate between subagents and consolidate their results
4. Execute git operations (add, commit, push) when code changes are complete
5. Provide a cohesive summary and next steps

Always:
- Use subagents for specialized work rather than doing everything yourself
- Provide clear context and objectives when delegating
- Monitor subagent progress and intervene if needed
- Summarize findings and integrate results from multiple subagents
- Commit and push changes when tasks are complete

Available subagents:
- @explore: Fast codebase exploration (read-only)
- @general: Multi-step tasks with full tool access
- @developer: Code implementation specialist
- @review: Code quality gate specialist
- @assist: Universal assistant for docs, git, and routine scripts

For git operations:
- Use bash tool with standard git commands (git push, git pull, git commit, etc.)
- Always check git status before committing
- Provide clear commit messages
- Push changes after successful commits
EOF

echo -e "${GREEN}✓${NC} Architect agent created at $AGENT_DIR/architect.md"
echo ""

# Step 3: Check git configuration
echo -e "${YELLOW}[3/5]${NC} Checking git configuration..."
if command -v git &> /dev/null; then
    GIT_NAME=$(git config --global user.name || echo "")
    GIT_EMAIL=$(git config --global user.email || echo "")

    if [ -z "$GIT_NAME" ] || [ -z "$GIT_EMAIL" ]; then
        echo -e "${YELLOW}⚠${NC} Git is not fully configured"
        echo "Please configure git:"
        echo "  git config --global user.name 'Your Name'"
        echo "  git config --global user.email 'your@email.com'"
    else
        echo -e "${GREEN}✓${NC} Git is configured: $GIT_NAME <$GIT_EMAIL>"
    fi
else
    echo -e "${YELLOW}⚠${NC} Git is not installed"
fi
echo ""

# Step 4: Start Opencode server
echo -e "${YELLOW}[4/5]${NC} Starting Opencode server..."
echo "Command: opencode serve --hostname=$OPENCODE_HOST --port=$OPENCODE_PORT"
echo ""
echo "Press Ctrl+C to stop the server"
echo ""
echo -e "${GREEN}Starting...${NC}"
echo ""

opencode serve --hostname="$OPENCODE_HOST" --port="$OPENCODE_PORT" &
OPENCODE_PID=$!

# Wait for server to start
echo "Waiting for server to start..."
for i in {1..30}; do
    if curl -s "http://$OPENCODE_HOST:$OPENCODE_PORT/vcs" > /dev/null 2>&1; then
        echo -e "${GREEN}✓${NC} Opencode server is running!"
        break
    fi
    sleep 1
done

echo ""
echo "=========================================="
echo "Setup Complete!"
echo "=========================================="
echo ""
echo "Opencode Server:"
echo "  URL: http://$OPENCODE_HOST:$OPENCODE_PORT"
echo "  PID: $OPENCODE_PID"
echo ""
echo "Environment Variables (for your agent):"
echo "  export OPENCODE_BASE_URL=http://$OPENCODE_HOST:$OPENCODE_PORT"
echo "  export OPENCODE_TIMEOUT=300"
echo ""
echo "Next Steps:"
echo "  1. Start your agent application"
echo "  2. Test with: {\"tool\": \"opencode\", \"task\": \"list files in current directory\"}"
echo "  3. Check API docs: http://$OPENCODE_HOST:$OPENCODE_PORT/doc"
echo ""
echo "To stop the server:"
echo "  kill $OPENCODE_PID"
echo ""
echo "To run health check:"
echo "  curl http://$OPENCODE_HOST:$OPENCODE_PORT/vcs"
echo ""

# Keep the script running
wait $OPENCODE_PID
