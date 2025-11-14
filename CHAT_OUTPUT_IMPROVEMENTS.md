# Chat Output & UX Improvements

## Summary

Implemented several key improvements to make chat output cleaner and improve the overall user experience:

1. **Suppress duplicate tool indicators** - No more repeated "Checking files..." messages
2. **Auto-format file paths** - File paths automatically displayed in monospace code blocks
3. **Delete conversations** - Added ability to delete conversations with confirmation
4. **Project selector page** - When visiting `/code/dashboard` without a project, shows all projects to choose from
5. **Auto-navigate to recent conversation** - Selecting a project automatically opens the most recent conversation
6. **Smart auto-scroll** - Only auto-scrolls if you're already at the bottom, so you can read at your own pace

---

## 1. Suppress Duplicate Tool Indicators

### Problem
When Augment uses the same tool multiple times in a row (e.g., `view` to check multiple files), the chat would show:
```
🔍 Checking files...
🔍 Checking files...
🔍 Checking files...
🔍 Checking files...
```

This was noisy and repetitive.

### Solution
Added duplicate detection to `StreamingToolCallFilter`:
- Tracks the last displayed tool name
- Only shows tool indicator if it's different from the previous one
- Consecutive uses of the same tool are silent

### Result
Now you see:
```
🔍 Checking files...
[All file checks happen silently]
```

Much cleaner! The first use shows what's happening, subsequent uses are hidden.

### Implementation
**File:** `src/codeforge/services/tool_call_parser.py`

Added tracking:
```python
self.last_displayed_tool = None  # Track last tool to avoid duplicates
```

Updated display logic:
```python
if self.current_tool_name != self.last_displayed_tool:
    display_text = self._get_tool_display(self.current_tool_name)
    yield f"\n{display_text}\n"
    self.last_displayed_tool = self.current_tool_name
```

---

## 2. Auto-Format File Paths in Monospace

### Problem
File paths in responses were displayed in regular text:
```
src/codeforge/config.py
src/codeforge/database.py
```

This made them hard to distinguish from regular text and didn't look like code.

### Solution
Added automatic file path detection and formatting in the frontend:
- Detects common file path patterns (e.g., `src/path/to/file.ext`, `/absolute/path.py`)
- Automatically wraps them in backticks for markdown rendering
- Results in monospace, highlighted code formatting

### Result
File paths now render as:
```
`src/codeforge/config.py`
`src/codeforge/database.py`
```

With monospace font and code block styling (green background, border).

### Implementation
**File:** `src/codeforge/static/js/dashboard.js`

Added new function with improved patterns:
```javascript
autoFormatFilePaths(content) {
    // Pattern 1: Absolute paths (with or without extensions)
    // Matches: /home/brandon/projects, /home/user/file.txt, /var/log/app.log
    content = content.replace(/(?<!`)(\b\/[a-zA-Z0-9_\-\/\.]+(?:\.[a-zA-Z0-9]{1,6})?\b)(?!`)/g, '`$1`');

    // Pattern 2: Relative paths with ./ prefix (with or without extensions)
    // Matches: ./README.md, ./src/file.js, ./config
    content = content.replace(/(?<!`)(\b\.\/[a-zA-Z0-9_\-\/\.]+(?:\.[a-zA-Z0-9]{1,6})?\b)(?!`)/g, '`$1`');

    // Pattern 3: Common project paths (src/, config/, etc.)
    // Matches: src/components/App.js, config/settings.json
    content = content.replace(/(?<!`)(\b(?:src|config|lib|dist|build)\/[a-zA-Z0-9_\-\/\.]+(?:\.[a-zA-Z0-9]{1,6})?\b)(?!`)/g, '`$1`');

    // Pattern 4: Common uppercase files with extensions (README.md, DEPLOYMENT.md, etc.)
    // Matches: README.md, CAPABILITIES.md, PROJECT_SUMMARY.md
    content = content.replace(/(?<!`)(\b[A-Z][A-Z0-9_\-]*\.[a-zA-Z0-9]{1,6}\b)(?!`)/g, '`$1`');
    // Only wrap if not already wrapped in backticks
    return content.replace(/(?<!`)((?:src|\.\/|\/|[a-zA-Z0-9_-]+\/)[a-zA-Z0-9_\-\/\.]+\.[a-z]{1,4})(?!`)/g, '`$1`');
}
```

Integrated into markdown rendering:
```javascript
renderMarkdown(content) {
    content = this.stripAnsiCodes(content);
    content = this.autoFormatFilePaths(content);  // ← New step
    let html = marked.parse(content);
    // ... rest of rendering
}
```

### Pattern Details
The regex matches:
- Paths starting with `src/`, `./`, `/`, or `dirname/`
- Containing alphanumeric characters, hyphens, underscores, slashes, dots
- Ending with a file extension (1-4 characters)
- Not already wrapped in backticks (negative lookbehind/lookahead)

---

## Combined Effect

**Before:**
```
Let me examine the key configuration files to understand the tech stack:
🔧 Using view...
🔧 Using view...
🔧 Using view...
🔧 Using view...
Here's the files and directories up to 2 levels deep in src/codeforge:
src/codeforge/init.py
src/codeforge/pycache
src/codeforge/auth.py
src/codeforge/config.py
```

**After:**
```
Let me examine the key configuration files to understand the tech stack:
🔍 Checking files...
Here's the files and directories up to 2 levels deep in src/codeforge:
`src/codeforge/init.py`
`src/codeforge/pycache`
`src/codeforge/auth.py`
`src/codeforge/config.py`
```

---

## Files Modified

1. **src/codeforge/services/tool_call_parser.py**
   - Added `last_displayed_tool` tracking
   - Updated tool display logic to suppress duplicates

2. **src/codeforge/static/js/dashboard.js**
   - Added `autoFormatFilePaths()` function
   - Added `autoFormatCodeSnippets()` function
   - Integrated into `renderMarkdown()` pipeline
   - Added `deleteConversation()` method
   - Added `showProjectSelector` state
   - Added `selectProjectFromSelector()` method
   - Modified `selectInitialProject()` to show project selector when no project in URL

3. **src/codeforge/templates/dashboard.html**
   - Added delete button to conversation list items
   - Added project selector page UI
   - Conditionally hide sidebar and main content when project selector is shown

---

## Testing

To see the improvements:
1. Start a new conversation
2. Ask Augment to check multiple files (e.g., "Show me the main config files")
3. Observe:
   - Only ONE "🔍 Checking files..." indicator
   - All file paths in monospace with code styling

---

---

## 3. Delete Conversations

### Feature
Added ability to delete conversations with a trash icon button.

### Implementation
**File:** `src/codeforge/templates/dashboard.html`

Added delete button next to edit button:
```html
<button @click.stop="deleteConversation(conv.id)"
    class="opacity-0 group-hover:opacity-100 text-gray-500 hover:text-red-400 transition-all p-1">
    <i class="fas fa-trash"></i>
</button>
```

**File:** `src/codeforge/static/js/dashboard.js`

Added delete method with confirmation:
```javascript
async deleteConversation(conversationId) {
    if (!confirm('Are you sure you want to delete this conversation? This cannot be undone.')) {
        return;
    }

    const response = await apiRequest(`${BASE_PATH}/api/chat/conversations/${conversationId}`, {
        method: 'DELETE'
    });

    if (response && response.ok) {
        // Remove from conversations list
        this.conversations = this.conversations.filter(c => c.id !== conversationId);

        // If we deleted the current conversation, select another one
        if (this.currentConversationId === conversationId) {
            if (this.conversations.length > 0) {
                await this.selectConversation(this.conversations[0].id);
            } else {
                // No conversations left, create a new one
                await this.createDefaultConversation();
            }
        }
    }
}
```

### User Experience
- Hover over a conversation to see edit and delete buttons
- Delete button shows in red when hovered
- Confirmation dialog prevents accidental deletion
- If you delete the current conversation, automatically switches to another one
- If no conversations remain, creates a new default conversation

---

## 4. Project Selector Page

### Feature
When visiting `https://forge-freedom.com/code/dashboard` without a `?project=X` parameter, shows a beautiful project selection page instead of auto-selecting the first project.

### Implementation
**File:** `src/codeforge/static/js/dashboard.js`

Added state and logic:
```javascript
showProjectSelector: false, // Show project selection page when no project is selected

async selectInitialProject() {
    const urlParams = new URLSearchParams(window.location.search);
    const urlProjectId = urlParams.get('project');

    if (urlProjectId) {
        // Load the specified project
        const project = this.projects.find(p => p.id === parseInt(urlProjectId));
        if (project) {
            this.currentProjectId = parseInt(urlProjectId);
            this.showProjectSelector = false;
            await this.switchProject();
            return;
        }
    }

    // No project specified in URL - show project selector
    if (!urlProjectId && this.projects.length > 0) {
        this.showProjectSelector = true;
        return;
    }
}

async selectProjectFromSelector(projectId) {
    this.currentProjectId = projectId;
    this.showProjectSelector = false;

    // Load conversations to find the most recent one
    await this.loadConversations();

    // Select the most recent conversation (first in the list since they're sorted by updated_at desc)
    if (this.conversations.length > 0) {
        const mostRecentConversation = this.conversations[0];
        this.updateUrl(projectId, mostRecentConversation.id, true);
        await this.selectConversation(mostRecentConversation.id);
    } else {
        // No conversations, just switch to the project
        this.updateUrl(projectId, null, true);
        await this.switchProject();
    }
}
```

**File:** `src/codeforge/templates/dashboard.html`

Added project selector UI:
```html
<!-- Project Selector Page (shown when no project is selected) -->
<div x-show="showProjectSelector" class="flex-1 flex items-center justify-center bg-gray-950 p-8">
    <div class="max-w-4xl w-full">
        <div class="text-center mb-8">
            <h2 class="text-3xl font-bold text-white mb-2 flex items-center justify-center gap-3">
                <i class="fas fa-folder-open text-forge-green-400"></i>
                Select a Project
            </h2>
            <p class="text-gray-400">Choose a project to continue to your most recent conversation</p>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
            <template x-for="project in projects" :key="project.id">
                <div @click="selectProjectFromSelector(project.id)"
                    class="bg-gray-900 border border-gray-800 rounded-lg p-6 cursor-pointer hover:border-forge-green-500 hover:bg-gray-800 transition-all hover:shadow-lg hover:shadow-forge-green-900/20 group">
                    <div class="flex items-start gap-3">
                        <div class="text-forge-green-400 text-2xl group-hover:scale-110 transition-transform">
                            <i class="fas fa-code-branch"></i>
                        </div>
                        <div class="flex-1 min-w-0">
                            <h3 class="text-white font-semibold text-lg mb-1 truncate" x-text="project.name"></h3>
                            <p class="text-gray-500 text-sm truncate" x-text="project.path"></p>
                            <p class="text-gray-600 text-xs mt-2" x-text="project.description || 'No description'"></p>
                        </div>
                    </div>
                </div>
            </template>
        </div>
    </div>
</div>
```

### User Experience
1. Visit `https://forge-freedom.com/code/dashboard` (no project parameter)
2. See a beautiful grid of all your projects
3. Click on a project
4. Automatically navigate to the most recent conversation for that project
5. URL updates to include both project and conversation IDs

---

## 5. Smart Auto-Scroll

### Problem
Previously, the chat would force-scroll to the bottom on every chunk of streaming output, making it impossible to read earlier messages while new content was being generated.

### Solution
Implemented smart auto-scroll that only scrolls if you're already near the bottom (within 100px).

### Implementation
**File:** `src/codeforge/static/js/dashboard.js`

Modified `scrollToBottom()` method:
```javascript
scrollToBottom(force = false) {
    const container = document.getElementById('messagesContainer');
    if (!container) return;

    // Only auto-scroll if user is already near the bottom (within 100px)
    // or if force is true (e.g., when loading messages or sending a new message)
    const isNearBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 100;

    if (force || isNearBottom) {
        container.scrollTop = container.scrollHeight;
    }
}
```

Updated WebSocket message handler:
```javascript
if (data.type === 'user_message') {
    this.messages.push(data.message);
    this.$nextTick(() => this.scrollToBottom(true)); // Force scroll for new user message
} else if (data.type === 'assistant_chunk') {
    this.isStreaming = true;
    this.streamingContent += data.chunk;
    this.$nextTick(() => this.scrollToBottom()); // Auto-scroll only if near bottom
} else if (data.type === 'assistant_complete') {
    this.isStreaming = false;
    this.messages.push(data.message);
    this.streamingContent = '';
    this.$nextTick(() => this.scrollToBottom()); // Auto-scroll only if near bottom
}
```

### User Experience
- **If you're at the bottom:** Chat auto-scrolls as new content streams in (normal behavior)
- **If you scroll up to read:** Chat stays where you are, doesn't fight you
- **When you send a message:** Always scrolls to bottom (force=true)
- **When you load a conversation:** Always scrolls to bottom (force=true)

This gives you full control to read at your own pace while still providing the convenience of auto-scroll when you're following along!

---

## Future Enhancements

Potential improvements:
- Detect and format other code elements (function names, class names, variables)
- Add syntax highlighting to inline code snippets
- Group tool calls by category (e.g., "🔍 Checking 4 files...")
- Add project search/filter on the project selector page
- Show conversation count and last activity date on project cards
- Add a "scroll to bottom" button that appears when you're scrolled up

