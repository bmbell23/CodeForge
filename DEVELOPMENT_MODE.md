# CodeForge Development Mode (Without Augment CLI)

If you want to develop and test CodeForge **without** installing the actual Augment CLI, you can use the built-in mock service.

## What You Get

The mock service simulates Augment CLI responses, allowing you to:
- ✅ Test the entire application flow
- ✅ Develop and debug the UI
- ✅ Test WebSocket communication
- ✅ Verify database operations
- ✅ Test project and file management
- ✅ Work on the frontend without backend dependencies

## Quick Start (Mock Mode)

### 1. Setup

```bash
cd ~/projects/CodeForge

# Run setup
./setup.sh

# The .env file will have USE_MOCK_AUGMENT=true by default
```

### 2. Verify Configuration

Check your `.env` file:

```bash
cat .env | grep USE_MOCK_AUGMENT
```

Should show:
```
USE_MOCK_AUGMENT=true
```

### 3. Create User and Start

```bash
# Create a user
python scripts/create_user.py brandon brandon@example.com password

# Start the server
python scripts/server.py
```

### 4. Use the Application

1. Open `http://localhost:8004`
2. Login with your credentials
3. Add projects
4. Create conversations
5. Chat with the **mock** Augment service

## Mock Service Behavior

The mock service will:
- Echo back your question
- Provide a simulated helpful response
- Stream the response word-by-word (like the real Augment)
- Save messages to the database
- Work with all project contexts

Example mock response:
```
I understand you asked: "How do I add a new feature?"

Here's a mock response from the Augment AI assistant:

This is a simulated response since the actual Augment CLI is not installed.
In a real scenario, I would analyze your codebase and provide specific,
context-aware suggestions.

For now, I can help you understand that:
- Your project is located at: my-project
- You asked: How do I add a new feature?
- This is a mock response for development/testing

To use the real Augment CLI:
1. Install it: npm install -g @augmentcode/auggie
2. Login: auggie login
3. Update .env: USE_MOCK_AUGMENT=false

Would you like me to help with anything else?
```

## Switching to Real Augment CLI

When you're ready to use the actual Augment CLI:

### 1. Install Augment CLI

```bash
npm install -g @augmentcode/auggie
auggie login
```

### 2. Update Configuration

Edit `.env`:

```bash
# Change this line:
USE_MOCK_AUGMENT=false
```

### 3. Restart Server

```bash
# Stop the server (Ctrl+C)
# Start it again
python scripts/server.py
```

Now CodeForge will use the real Augment CLI! 🎉

## Switching Back to Mock

If you want to go back to mock mode (for testing, development, or if auggie is having issues):

```bash
# Edit .env
USE_MOCK_AUGMENT=true

# Restart server
python scripts/server.py
```

## Development Workflow

### Recommended Approach

1. **Start with Mock**: Develop UI and features using mock service
2. **Test with Real**: Once features work, test with real Augment CLI
3. **Debug with Mock**: If issues arise, switch back to mock to isolate problems

### Benefits of Mock Mode

- **No External Dependencies**: Work offline or without npm/node
- **Faster Responses**: No network latency
- **Predictable Behavior**: Same responses every time
- **Easy Debugging**: Know exactly what the "AI" will say
- **Cost-Free**: No API calls or usage limits

### When to Use Real Augment

- **Final Testing**: Before deploying or sharing
- **Feature Validation**: Ensure real AI responses work as expected
- **Demo/Production**: When showing to users or in production
- **Real Coding**: When you actually want AI coding assistance

## Customizing Mock Responses

You can customize the mock service to return different responses:

Edit `src/codeforge/services/mock_augment_service.py`:

<augment_code_snippet path="CodeForge/src/codeforge/services/mock_augment_service.py" mode="EXCERPT">
````python
async def stream_response(self, prompt: str) -> AsyncGenerator[str, None]:
    # Customize this response!
    response = f"Your custom response to: {prompt}"
    
    # ... rest of the code
````
</augment_code_snippet>

## Troubleshooting

### Mock Not Working?

Check `.env`:
```bash
grep USE_MOCK_AUGMENT .env
```

Should be `true` (lowercase).

### Still Getting Augment Errors?

1. Restart the server
2. Check server logs for errors
3. Verify `mock_augment_service.py` exists
4. Check browser console for errors

### Want to Test Both?

You can quickly switch between mock and real:

```bash
# Use mock
echo "USE_MOCK_AUGMENT=true" >> .env

# Use real
echo "USE_MOCK_AUGMENT=false" >> .env

# Restart server each time
python scripts/server.py
```

## Summary

**TL;DR:**
- By default, CodeForge uses a **mock** Augment service
- No need to install `auggie` to develop and test
- Set `USE_MOCK_AUGMENT=false` in `.env` when you want real AI
- Switch back and forth anytime by changing the config

This makes CodeForge easy to develop, test, and demo without external dependencies! 🚀

