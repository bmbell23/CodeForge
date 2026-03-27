// BASE_PATH is already defined in base.html, so we don't redeclare it here

// Cache helper functions
const CACHE_KEYS = {
    USER: 'codeforge_user',
    PROJECTS: 'codeforge_projects',
    LAST_PROJECT: 'codeforge_last_project',
    LAST_CONVERSATION: 'codeforge_last_conversation'
};

const CACHE_DURATION = 5 * 60 * 1000; // 5 minutes

function getCached(key) {
    try {
        const cached = localStorage.getItem(key);
        if (!cached) return null;

        const { data, timestamp } = JSON.parse(cached);
        if (Date.now() - timestamp > CACHE_DURATION) {
            localStorage.removeItem(key);
            return null;
        }
        return data;
    } catch (e) {
        console.error('Cache read error:', e);
        return null;
    }
}

function setCache(key, data) {
    try {
        localStorage.setItem(key, JSON.stringify({
            data,
            timestamp: Date.now()
        }));
    } catch (e) {
        console.error('Cache write error:', e);
    }
}

function clearCache(key) {
    try {
        if (key) {
            localStorage.removeItem(key);
        } else {
            // Clear all CodeForge caches
            Object.values(CACHE_KEYS).forEach(k => localStorage.removeItem(k));
        }
    } catch (e) {
        console.error('Cache clear error:', e);
    }
}

function dashboardApp() {
    return {
        // User data
        username: '',

        // Projects
        projects: [],
        currentProjectId: '',
        scannedProjects: [],
        showProjectScan: false,
        renamingProjectId: null,
        renamingProjectName: '',
        loadingProjects: false,

        // Conversations
        conversations: [],
        currentConversationId: null,
        messages: [],
        editingConversationId: null,
        editingConversationTitle: '',

        // Chat
        messageInput: '',
        isStreaming: false,
        streamingContent: '',
        ws: null,
        attachedImages: [],

        // UI
        currentTab: 'chat',
        sidebarOpen: false,
        filePickerOpen: true, // File picker visibility on mobile
        showProjectSelector: false, // Show project selection page when no project is selected

        // Files
        fileTree: [],
        currentFile: null,
        fileContent: '',
        currentPath: '', // Track current directory path
        loadingFileTree: false,

        // Git
        gitStatus: null,
        gitLog: [],
        quickCommitMessage: '',
        quickCommitLoading: false,
        quickCommitResult: '',
        quickCommitSuccess: false,

        // Terminal - store per project
        terminals: {}, // Map of project_id -> {terminal, fitAddon, ws}
        currentTerminalProjectId: null,

        // Password change
        passwordForm: {
            current: '',
            new: '',
            confirm: ''
        },
        passwordError: '',
        passwordSuccess: '',

        async init() {
            // Show loading state
            console.log('=== CodeForge Initialization Started ===');
            console.log('User agent:', navigator.userAgent);
            console.log('Is mobile:', navigator.userAgent.includes('Mobile'));
            console.log('BASE_PATH:', window.BASE_PATH || 'undefined');
            console.log('Window location:', window.location.href);
            console.log('Local storage token exists:', !!localStorage.getItem('token'));
            console.log('Alpine.js version:', window.Alpine?.version || 'not loaded');

            try {
                // Test basic API connectivity
                await this.testApiConnectivity();

                await this.loadUser();
                console.log('User loaded, username:', this.username);

                await this.autoLoadAllProjects(); // Auto-scan and load all projects
                console.log('Projects after loading:', this.projects.length);

                this.configureMarked();
                this.initMobileHandlers();
                this.initUrlStateManagement();

                // Handle window resize for terminal
                window.addEventListener('resize', () => {
                    if (this.currentTerminalProjectId && this.terminals[this.currentTerminalProjectId]) {
                        const terminalData = this.terminals[this.currentTerminalProjectId];
                        terminalData.fitAddon.fit();
                    }
                });

                // Watch for tab changes to ensure file tree is loaded
                this.$watch('currentTab', (newTab) => {
                    if (newTab === 'files' && this.currentProjectId && this.fileTree.length === 0) {
                        console.log('Files tab activated, loading file tree');
                        this.loadFileTree();
                    }
                });

                console.log('=== CodeForge Initialization Complete ===');
                console.log('Final state - projects:', this.projects.length, 'currentProjectId:', this.currentProjectId, 'showProjectSelector:', this.showProjectSelector);

                // Add a global debug function for mobile testing
                window.debugCodeForge = () => {
                    console.log('=== Debug Info ===');
                    console.log('Projects:', this.projects);
                    console.log('Current Project ID:', this.currentProjectId);
                    console.log('Show Project Selector:', this.showProjectSelector);
                    console.log('Username:', this.username);
                    console.log('Loading Projects:', this.loadingProjects);
                    console.log('Cache:', {
                        projects: getCached(CACHE_KEYS.PROJECTS),
                        user: getCached(CACHE_KEYS.USER),
                        lastProject: getCached(CACHE_KEYS.LAST_PROJECT)
                    });
                };

                // Add a global function to force load test projects (for debugging)
                window.forceTestProjects = () => {
                    console.log('Forcing test projects...');
                    this.projects = [
                        { id: 1, name: 'Test Project 1', path: 'test1', description: 'Test project 1' },
                        { id: 2, name: 'Test Project 2', path: 'test2', description: 'Test project 2' },
                        { id: 3, name: 'CodeForge', path: 'CodeForge', description: 'CodeForge project' }
                    ];
                    this.showProjectSelector = true;
                    console.log('Test projects loaded:', this.projects);
                };

            } catch (error) {
                console.error('Error during CodeForge initialization:', error);
                console.error('Error stack:', error.stack);

                // On mobile, show a more visible error
                if (navigator.userAgent.includes('Mobile')) {
                    alert('CodeForge initialization failed. Check console for details.');
                }
            }
        },

        initUrlStateManagement() {
            // Parse URL parameters on load
            this.parseUrlState();

            // Handle browser back/forward navigation
            window.addEventListener('popstate', (event) => {
                if (event.state) {
                    this.restoreStateFromUrl(event.state);
                } else {
                    this.parseUrlState();
                }
            });
        },

        parseUrlState() {
            const urlParams = new URLSearchParams(window.location.search);
            const projectId = urlParams.get('project');
            const conversationId = urlParams.get('conversation');

            if (projectId) {
                this.currentProjectId = parseInt(projectId);
            }

            if (conversationId) {
                this.currentConversationId = parseInt(conversationId);
            }
        },

        updateUrl(projectId = null, conversationId = null, replaceState = false) {
            const url = new URL(window.location);

            // Update project parameter
            if (projectId !== null) {
                if (projectId) {
                    url.searchParams.set('project', projectId);
                } else {
                    url.searchParams.delete('project');
                }
            }

            // Update conversation parameter
            if (conversationId !== null) {
                if (conversationId) {
                    url.searchParams.set('conversation', conversationId);
                } else {
                    url.searchParams.delete('conversation');
                }
            }

            // Update browser history
            const state = {
                projectId: this.currentProjectId,
                conversationId: this.currentConversationId
            };

            if (replaceState) {
                window.history.replaceState(state, '', url.toString());
            } else {
                window.history.pushState(state, '', url.toString());
            }
        },

        async restoreStateFromUrl(state) {
            if (state.projectId && state.projectId !== this.currentProjectId) {
                this.currentProjectId = state.projectId;
                await this.switchProject();
            }

            if (state.conversationId && state.conversationId !== this.currentConversationId) {
                await this.selectConversation(state.conversationId);
            }
        },

        configureMarked() {
            // Configure marked.js for better rendering
            if (typeof marked !== 'undefined') {
                marked.setOptions({
                    breaks: true,
                    gfm: true,
                    highlight: function(code, lang) {
                        if (typeof hljs !== 'undefined' && lang && hljs.getLanguage(lang)) {
                            try {
                                return hljs.highlight(code, { language: lang }).value;
                            } catch (err) {}
                        }
                        return code;
                    }
                });
            }
        },

        stripAnsiCodes(text) {
            // Remove ANSI escape codes (terminal color codes)
            // Pattern matches: ESC[...m or ESC[...
            return text.replace(/\x1b\[[0-9;]*m/g, '')
                      .replace(/\x1b\[[0-9;]*[A-Za-z]/g, '')
                      .replace(/\[\d+m/g, '');
        },

        autoFormatFilePaths(content) {
            // Auto-wrap file paths in backticks for monospace rendering
            // Match common file path patterns (but not if already in backticks or code blocks)

            // Don't process if already in code block
            if (content.includes('```')) {
                return content;
            }

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

            return content;
        },

        autoFormatCodeSnippets(content) {
            // Auto-wrap code snippets (numbered lines with code) in code blocks
            const lines = content.split('\n');
            let inCodeBlock = false;
            let codeBlockStart = -1;
            let result = [];

            for (let i = 0; i < lines.length; i++) {
                const line = lines[i];
                const trimmed = line.trim();

                // Check if line starts with a number (line number from tool output)
                const isNumberedLine = /^\d+\s/.test(trimmed);

                // Check if this looks like code output header
                const isCodeHeader = trimmed.startsWith('Path:') ||
                                   trimmed.includes('"""') ||
                                   trimmed.startsWith('The following code sections were retrieved:');

                if (isNumberedLine && !inCodeBlock) {
                    // Start of code block
                    inCodeBlock = true;
                    codeBlockStart = i;
                    result.push('```python');
                    result.push(line);
                } else if (inCodeBlock && !isNumberedLine && trimmed !== '' && !trimmed.startsWith('...')) {
                    // End of code block
                    result.push('```');
                    result.push(line);
                    inCodeBlock = false;
                } else if (inCodeBlock && trimmed === '') {
                    // Empty line in code block - might be end
                    // Look ahead to see if more numbered lines follow
                    let hasMoreCode = false;
                    for (let j = i + 1; j < Math.min(i + 3, lines.length); j++) {
                        if (/^\d+\s/.test(lines[j].trim())) {
                            hasMoreCode = true;
                            break;
                        }
                    }
                    if (hasMoreCode) {
                        result.push(line);
                    } else {
                        result.push('```');
                        result.push(line);
                        inCodeBlock = false;
                    }
                } else {
                    result.push(line);
                }
            }

            // Close any open code block
            if (inCodeBlock) {
                result.push('```');
            }

            return result.join('\n');
        },

        renderMarkdown(content) {
            if (!content) return '';

            // Strip ANSI codes first
            content = this.stripAnsiCodes(content);

            // Auto-format code snippets (numbered lines)
            content = this.autoFormatCodeSnippets(content);

            // Auto-format file paths
            content = this.autoFormatFilePaths(content);

            if (typeof marked === 'undefined') {
                // Fallback if marked.js isn't loaded
                return content.replace(/\n/g, '<br>');
            }
            try {
                // Parse markdown
                let html = marked.parse(content);

                // Apply syntax highlighting to any code blocks that weren't highlighted
                if (typeof hljs !== 'undefined') {
                    const tempDiv = document.createElement('div');
                    tempDiv.innerHTML = html;
                    tempDiv.querySelectorAll('pre code:not(.hljs)').forEach((block) => {
                        hljs.highlightElement(block);
                    });
                    html = tempDiv.innerHTML;
                }

                return html;
            } catch (err) {
                console.error('Markdown rendering error:', err);
                return content.replace(/\n/g, '<br>');
            }
        },

        async testApiConnectivity() {
            try {
                console.log('Testing API connectivity...');
                const healthUrl = `${BASE_PATH}/health`;
                console.log('Health check URL:', healthUrl);

                const response = await fetch(healthUrl);
                console.log('Health check response:', response.status, response.ok);

                if (response.ok) {
                    const data = await response.json();
                    console.log('Health check data:', data);

                    // Test if we can reach the projects API without auth (should fail with 401)
                    try {
                        const projectsUrl = `${BASE_PATH}/api/projects/`;
                        console.log('Testing projects API (should fail with 401):', projectsUrl);
                        const projectsResponse = await fetch(projectsUrl);
                        console.log('Projects API response (no auth):', projectsResponse.status, projectsResponse.statusText);
                    } catch (projectsError) {
                        console.log('Projects API error (expected):', projectsError);
                    }
                } else {
                    console.error('Health check failed:', response.status, response.statusText);
                }
            } catch (error) {
                console.error('Health check error:', error);
            }
        },

        async loadUser() {
            // Try cache first
            const cached = getCached(CACHE_KEYS.USER);
            if (cached) {
                this.username = cached.username;
                console.log('Loaded user from cache:', this.username);
                // Still fetch in background to update cache
                this.loadUserFromAPI();
                return;
            }

            await this.loadUserFromAPI();
        },

        async loadUserFromAPI() {
            try {
                const response = await apiRequest(`${BASE_PATH}/api/auth/me`);
                console.log('User API response:', response?.status, response?.ok);
                if (response && response.ok) {
                    const user = await response.json();
                    console.log('User data received:', user);
                    this.username = user.username;
                    setCache(CACHE_KEYS.USER, user);
                } else {
                    console.error('Failed to load user:', response?.status, response?.statusText);
                    if (response) {
                        const errorText = await response.text();
                        console.error('User API error details:', errorText);
                    }
                }
            } catch (error) {
                console.error('Error loading user:', error);
            }
        },

        async autoLoadAllProjects() {
            console.log('Auto-loading all projects...');
            this.loadingProjects = true;

            try {
                // Try cache first for faster initial load
                const cached = getCached(CACHE_KEYS.PROJECTS);
                if (cached && cached.length > 0) {
                    this.projects = cached;
                    console.log('Loaded projects from cache:', this.projects.length);

                    // Select project from URL or last project
                    await this.selectInitialProject();

                    // Still scan and update in background
                    this.scanAndUpdateProjects();
                    return;
                }

                // No cache, do full scan with retry
                await this.scanAndUpdateProjectsWithRetry();
            } finally {
                this.loadingProjects = false;
            }
        },

        async scanAndUpdateProjectsWithRetry(maxRetries = 3) {
            for (let attempt = 1; attempt <= maxRetries; attempt++) {
                try {
                    await this.scanAndUpdateProjects();
                    // If we get here, it succeeded
                    return;
                } catch (error) {
                    console.error(`Project loading attempt ${attempt} failed:`, error);
                    if (attempt === maxRetries) {
                        console.error('All project loading attempts failed, showing project selector');
                        this.showProjectSelector = true;
                    } else {
                        // Wait before retrying (exponential backoff)
                        const delay = Math.pow(2, attempt - 1) * 1000; // 1s, 2s, 4s
                        console.log(`Retrying in ${delay}ms...`);
                        await new Promise(resolve => setTimeout(resolve, delay));
                    }
                }
            }
        },

        async scanAndUpdateProjects() {
            try {
                // First, scan for all projects in the directory
                const scanResponse = await apiRequest(`${BASE_PATH}/api/projects/scan`);
                if (!scanResponse || !scanResponse.ok) {
                    console.error('Failed to scan projects, falling back to loading existing projects');
                    // Fallback to just loading existing projects from DB
                    await this.loadProjects();
                    return;
                }

                const scannedProjects = await scanResponse.json();
                console.log('Scanned projects:', scannedProjects.length);

                // Auto-add any projects that don't exist in DB yet
                for (const project of scannedProjects) {
                    if (!project.exists_in_db) {
                        console.log('Auto-adding project:', project.name);
                        try {
                            const addResponse = await apiRequest(`${BASE_PATH}/api/projects/`, {
                                method: 'POST',
                                body: {
                                    name: project.name,
                                    path: project.path,
                                    description: ''
                                }
                            });
                            if (!addResponse || !addResponse.ok) {
                                console.error('Failed to add project:', project.name);
                            }
                        } catch (error) {
                            console.error('Error adding project:', project.name, error);
                        }
                    }
                }

                // Now load all projects from DB
                await this.loadProjects();
            } catch (error) {
                console.error('Error in scanAndUpdateProjects:', error);
                // Fallback to loading existing projects
                await this.loadProjects();
            }
        },

        async loadProjects() {
            console.log('loadProjects() called');
            try {
                const url = `${BASE_PATH}/api/projects/`;
                console.log('Making API request to:', url);
                const response = await apiRequest(url);
                console.log('API response:', response?.status, response?.ok);

                if (response && response.ok) {
                    this.projects = await response.json();
                    console.log('Raw projects from API:', this.projects);
                    // Sort projects alphabetically by name
                    this.projects.sort((a, b) => a.name.localeCompare(b.name));
                    setCache(CACHE_KEYS.PROJECTS, this.projects);
                    console.log('Projects loaded and sorted:', this.projects.length, 'projects');

                    await this.selectInitialProject();
                } else {
                    console.error('Failed to load projects from API:', response?.status, response?.statusText);
                    if (response) {
                        const errorText = await response.text();
                        console.error('API error details:', errorText);
                    }
                    // If we have cached projects, use them as fallback
                    const cached = getCached(CACHE_KEYS.PROJECTS);
                    if (cached && cached.length > 0) {
                        console.log('Using cached projects as fallback:', cached.length);
                        this.projects = cached;
                        await this.selectInitialProject();
                    } else {
                        console.warn('No projects available and no cache found');
                        this.showProjectSelector = true;
                    }
                }
            } catch (error) {
                console.error('Error loading projects:', error);
                console.error('Error stack:', error.stack);
                // Try to use cached projects as fallback
                const cached = getCached(CACHE_KEYS.PROJECTS);
                if (cached && cached.length > 0) {
                    console.log('Using cached projects due to error:', cached.length);
                    this.projects = cached;
                    await this.selectInitialProject();
                } else {
                    console.warn('No projects available due to error and no cache found');
                    // Add a test project for debugging on mobile
                    if (navigator.userAgent.includes('Mobile')) {
                        console.log('Mobile detected, adding test project for debugging');
                        this.projects = [{
                            id: 999,
                            name: 'Test Project (Mobile Debug)',
                            path: '/test',
                            description: 'Debug project for mobile testing'
                        }];
                    }
                    this.showProjectSelector = true;
                }
            }
        },

        async selectInitialProject() {
            // Check if we have a project ID from URL
            const urlParams = new URLSearchParams(window.location.search);
            const urlProjectId = urlParams.get('project');

            if (urlProjectId) {
                // Validate that the project exists
                const project = this.projects.find(p => p.id === parseInt(urlProjectId));
                if (project) {
                    this.currentProjectId = parseInt(urlProjectId);
                    this.showProjectSelector = false;
                    await this.switchProject();
                    return;
                }
            }

            // Check if we have a cached last project to restore
            const cachedLastProject = getCached(CACHE_KEYS.LAST_PROJECT);
            if (cachedLastProject && this.projects.length > 0) {
                const lastProject = this.projects.find(p => p.id === cachedLastProject.id);
                if (lastProject) {
                    this.currentProjectId = lastProject.id;
                    this.showProjectSelector = false;
                    await this.switchProject();
                    return;
                }
            }

            // No project specified in URL and no cached project - show project selector
            if (!urlProjectId && this.projects.length > 0) {
                this.showProjectSelector = true;
                return;
            }

            // Fallback: select first project if no valid project from URL
            if (this.projects.length > 0 && !this.currentProjectId) {
                this.currentProjectId = this.projects[0].id;
                this.showProjectSelector = false;
                await this.switchProject();
            }
        },

        async selectProjectFromSelector(projectId) {
            this.currentProjectId = projectId;
            this.showProjectSelector = false;

            // Cache the selected project for future sessions
            const selectedProject = this.projects.find(p => p.id === projectId);
            if (selectedProject) {
                setCache(CACHE_KEYS.LAST_PROJECT, selectedProject);
            }

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
        },

        async switchProject() {
            if (!this.currentProjectId) return;

            console.log('Switching to project:', this.currentProjectId);

            // Cache the selected project for future sessions
            const currentProject = this.projects.find(p => p.id === this.currentProjectId);
            if (currentProject) {
                setCache(CACHE_KEYS.LAST_PROJECT, currentProject);
            }

            // Update URL with new project ID
            this.updateUrl(this.currentProjectId, null, true);

            // Load conversations for this project
            await this.loadConversations();

            // If no conversations exist, create a default one
            if (this.conversations.length === 0) {
                console.log('No conversations found, creating default conversation');
                await this.createDefaultConversation();
            }

            // Load file tree, git status in parallel for speed
            await Promise.all([
                this.loadFileTree(),
                this.loadGitStatus()
            ]);

            // If terminal tab is active, switch to the terminal for this project
            if (this.currentTab === 'terminal') {
                this.initTerminal();
            }

            // Switch to chat tab
            this.currentTab = 'chat';
        },

        async loadConversations() {
            const url = this.currentProjectId
                ? `${BASE_PATH}/api/chat/conversations?project_id=${this.currentProjectId}`
                : `${BASE_PATH}/api/chat/conversations`;

            const response = await apiRequest(url);
            if (response && response.ok) {
                this.conversations = await response.json();
                console.log('Loaded conversations:', this.conversations.length);

                // Check if we have a conversation ID from URL
                const urlParams = new URLSearchParams(window.location.search);
                const urlConversationId = urlParams.get('conversation');

                if (urlConversationId) {
                    // Validate that the conversation exists and belongs to current project
                    const conversation = this.conversations.find(c => c.id === parseInt(urlConversationId));
                    if (conversation) {
                        await this.selectConversation(parseInt(urlConversationId));
                        return;
                    }
                }

                // Fallback: select most recent conversation (first in list since they're ordered by updated_at desc)
                if (this.conversations.length > 0 && !this.currentConversationId) {
                    console.log('Auto-selecting first conversation');
                    await this.selectConversation(this.conversations[0].id);
                }
            }
        },

        async createDefaultConversation() {
            console.log('Creating default conversation for project:', this.currentProjectId);
            const response = await apiRequest(`${BASE_PATH}/api/chat/conversations`, {
                method: 'POST',
                body: {
                    title: 'New Chat',
                    project_id: this.currentProjectId
                }
            });

            if (response && response.ok) {
                const newConversation = await response.json();
                this.conversations.push(newConversation);
                console.log('Created default conversation:', newConversation.id);

                // Auto-select the new conversation
                await this.selectConversation(newConversation.id);
            }
        },

        async createConversation() {
            const response = await apiRequest(`${BASE_PATH}/api/chat/conversations`, {
                method: 'POST',
                body: {
                    title: 'New Conversation',
                    project_id: this.currentProjectId || null
                }
            });

            if (response && response.ok) {
                const conversation = await response.json();
                this.conversations.unshift(conversation);
                await this.selectConversation(conversation.id);
            }
        },

        async selectConversation(conversationId) {
            this.currentConversationId = conversationId;

            // Update URL with new conversation ID
            this.updateUrl(null, conversationId, true);

            // Close existing WebSocket (disable auto-reconnect first)
            if (this._wsReconnectTimer) {
                clearTimeout(this._wsReconnectTimer);
                this._wsReconnectTimer = null;
            }
            if (this.ws) {
                this._wsIntentionalClose = true;
                this.ws.close();
            }

            // Load messages
            await this.loadMessages();

            // Connect WebSocket
            this.connectWebSocket();

            // Close sidebar on mobile
            this.sidebarOpen = false;
        },

        startEditingConversation(conversationId, currentTitle) {
            this.editingConversationId = conversationId;
            this.editingConversationTitle = currentTitle;
            this.$nextTick(() => {
                const input = document.querySelector('input[x-model="editingConversationTitle"]');
                if (input) {
                    input.focus();
                    input.select();
                }
            });
        },

        async saveConversationTitle(conversationId) {
            if (!this.editingConversationTitle.trim()) {
                this.cancelEditingConversation();
                return;
            }

            const response = await apiRequest(`${BASE_PATH}/api/chat/conversations/${conversationId}`, {
                method: 'PUT',
                body: {
                    title: this.editingConversationTitle.trim()
                }
            });

            if (response && response.ok) {
                const updatedConv = await response.json();
                const index = this.conversations.findIndex(c => c.id === conversationId);
                if (index !== -1) {
                    this.conversations[index].title = updatedConv.title;
                }
            }

            this.editingConversationId = null;
            this.editingConversationTitle = '';
        },

        cancelEditingConversation() {
            this.editingConversationId = null;
            this.editingConversationTitle = '';
        },

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
        },

        async loadMessages() {
            const response = await apiRequest(`${BASE_PATH}/api/chat/conversations/${this.currentConversationId}/messages`);
            if (response && response.ok) {
                this.messages = await response.json();
                this.$nextTick(() => this.scrollToBottom(true)); // Force scroll when loading messages
            }
        },

        connectWebSocket() {
            if (!this.currentConversationId) {
                console.warn('Cannot connect WebSocket: no conversation selected');
                return;
            }

            // Clear any existing reconnect timer
            if (this._wsReconnectTimer) {
                clearTimeout(this._wsReconnectTimer);
                this._wsReconnectTimer = null;
            }

            const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
            const wsUrl = `${protocol}//${window.location.host}${BASE_PATH}/api/chat/ws/${this.currentConversationId}`;

            console.log('Connecting WebSocket to:', wsUrl);
            this._wsReconnectAttempts = 0;
            this.ws = new WebSocket(wsUrl);

            this.ws.onopen = () => {
                console.log('WebSocket connected successfully');
                this._wsReconnectAttempts = 0;
            };

            this.ws.onmessage = (event) => {
                const data = JSON.parse(event.data);

                if (data.type === 'user_message') {
                    // Avoid duplicate if we already have this message
                    if (!this.messages.find(m => m.id === data.message.id)) {
                        this.messages.push(data.message);
                    }
                    this.$nextTick(() => this.scrollToBottom(true));
                } else if (data.type === 'streaming_resumed') {
                    // Reconnected to an in-progress response - show what we have so far
                    console.log('Resumed streaming, content so far:', data.content_so_far?.length, 'chars');
                    this.isStreaming = true;
                    this.streamingContent = data.content_so_far || '';
                    this.$nextTick(() => this.scrollToBottom());
                } else if (data.type === 'assistant_chunk') {
                    this.isStreaming = true;
                    this.streamingContent += data.chunk;
                    this.$nextTick(() => this.scrollToBottom());
                } else if (data.type === 'assistant_complete') {
                    this.isStreaming = false;
                    if (data.message && !this.messages.find(m => m.id === data.message.id)) {
                        this.messages.push(data.message);
                    }
                    this.streamingContent = '';
                    this.$nextTick(() => this.scrollToBottom());
                } else if (data.error) {
                    console.error('WebSocket error:', data.error);
                    alert('Error: ' + data.error);
                }
            };

            this.ws.onerror = (error) => {
                console.error('WebSocket error:', error);
            };

            this.ws.onclose = (event) => {
                console.log('WebSocket closed', event.code, event.reason);
                // Don't auto-reconnect if we intentionally closed (e.g. switching conversations)
                if (this._wsIntentionalClose) {
                    this._wsIntentionalClose = false;
                    return;
                }
                // Auto-reconnect with exponential backoff
                if (this.currentConversationId) {
                    this._wsReconnectAttempts = (this._wsReconnectAttempts || 0) + 1;
                    const delay = Math.min(1000 * Math.pow(2, this._wsReconnectAttempts - 1), 30000);
                    console.log(`WebSocket reconnecting in ${delay}ms (attempt ${this._wsReconnectAttempts})`);
                    this._wsReconnectTimer = setTimeout(() => {
                        if (this.currentConversationId) {
                            // Reload messages first to pick up anything saved while disconnected
                            this.loadMessages().then(() => {
                                this.connectWebSocket();
                            });
                        }
                    }, delay);
                }
            };
        },

        sendMessage() {
            if ((!this.messageInput.trim() && this.attachedImages.length === 0) || !this.ws || this.isStreaming) return;
            if (this.ws.readyState !== WebSocket.OPEN) {
                console.warn('WebSocket not open, reconnecting...');
                this.connectWebSocket();
                return;
            }

            this.ws.send(JSON.stringify({
                message: this.messageInput,
                attachments: this.attachedImages.length > 0 ? this.attachedImages : null
            }));

            this.messageInput = '';
            this.attachedImages = [];
        },

        handleImageUpload(event) {
            const file = event.target.files[0];
            if (!file) return;

            // Check file size (limit to 5MB)
            if (file.size > 5 * 1024 * 1024) {
                alert('Image size must be less than 5MB');
                return;
            }

            const reader = new FileReader();
            reader.onload = (e) => {
                this.attachedImages.push({
                    name: file.name,
                    type: file.type,
                    data: e.target.result
                });
            };
            reader.readAsDataURL(file);

            // Reset input
            event.target.value = '';
        },

        removeImage(index) {
            this.attachedImages.splice(index, 1);
        },

        scrollToBottom(force = false) {
            const container = document.getElementById('messagesContainer');
            if (!container) return;

            // Only auto-scroll if user is already near the bottom (within 100px)
            // or if force is true (e.g., when loading messages or sending a new message)
            const isNearBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 100;

            if (force || isNearBottom) {
                container.scrollTop = container.scrollHeight;
            }
        },

        async scanProjects() {
            const response = await apiRequest(`${BASE_PATH}/api/projects/scan`);
            if (response && response.ok) {
                this.scannedProjects = await response.json();
                this.showProjectScan = true;
            }
        },

        async addProject(project) {
            const response = await apiRequest(`${BASE_PATH}/api/projects/`, {
                method: 'POST',
                body: {
                    name: project.name,
                    path: project.path,
                    description: ''
                }
            });

            if (response && response.ok) {
                const newProject = await response.json();
                this.projects.push(newProject);
                project.exists_in_db = true;

                // Auto-select the newly added project
                this.currentProjectId = newProject.id;
                await this.switchProject();

                // Close modal after a short delay
                setTimeout(() => {
                    this.showProjectScan = false;
                }, 500);
            }
        },

        startRenameProject(projectId) {
            console.log('Starting rename for project:', projectId);
            const project = this.projects.find(p => p.id === projectId);
            if (project) {
                this.renamingProjectId = projectId;
                this.renamingProjectName = project.name;
                console.log('Rename mode activated for:', project.name);
            }
        },

        cancelRenameProject() {
            this.renamingProjectId = null;
            this.renamingProjectName = '';
        },

        async confirmRenameProject() {
            if (!this.renamingProjectId || !this.renamingProjectName.trim()) {
                return;
            }

            const response = await apiRequest(`${BASE_PATH}/api/projects/${this.renamingProjectId}`, {
                method: 'PUT',
                body: {
                    name: this.renamingProjectName.trim(),
                    description: ''
                }
            });

            if (response && response.ok) {
                const updatedProject = await response.json();
                // Update the project in the local array
                const projectIndex = this.projects.findIndex(p => p.id === this.renamingProjectId);
                if (projectIndex !== -1) {
                    this.projects[projectIndex] = updatedProject;
                }
                this.cancelRenameProject();
                return true;
            } else {
                alert('Failed to rename project. Please try again.');
                return false;
            }
        },

        async loadFileTree(path = '') {
            if (!this.currentProjectId) {
                console.warn('No current project selected');
                return;
            }

            this.loadingFileTree = true;

            const url = path
                ? `${BASE_PATH}/api/files/${this.currentProjectId}/tree?path=${encodeURIComponent(path)}`
                : `${BASE_PATH}/api/files/${this.currentProjectId}/tree`;

            try {
                const response = await apiRequest(url);
                if (response && response.ok) {
                    this.fileTree = await response.json();
                    this.currentPath = path;
                    console.log('File tree loaded successfully for path:', path);
                } else {
                    console.error('Failed to load file tree:', response?.status, response?.statusText);
                    if (response) {
                        const errorText = await response.text();
                        console.error('Error details:', errorText);
                        alert(`Failed to load folder: ${response.status} ${response.statusText}`);
                    }
                }
            } catch (error) {
                console.error('Error loading file tree:', error);
                alert(`Error loading folder: ${error.message}`);
            } finally {
                this.loadingFileTree = false;
            }
        },

        async openFile(path) {
            if (!this.currentProjectId) return;

            const response = await apiRequest(`${BASE_PATH}/api/files/${this.currentProjectId}/content?path=${encodeURIComponent(path)}`);
            if (response && response.ok) {
                const data = await response.json();
                this.currentFile = path;
                this.fileContent = data.content;
            }
        },

        async navigateToFolder(folderPath) {
            if (!this.currentProjectId) {
                console.warn('No current project selected for folder navigation');
                return;
            }

            console.log('Navigating to folder:', folderPath, 'from current path:', this.currentPath);

            // Build the new path
            const newPath = this.currentPath
                ? `${this.currentPath}/${folderPath}`
                : folderPath;

            console.log('New path will be:', newPath);
            await this.loadFileTree(newPath);
        },

        async navigateUp() {
            if (!this.currentPath) return; // Already at root

            // Get parent path
            const pathParts = this.currentPath.split('/');
            pathParts.pop(); // Remove last part
            const parentPath = pathParts.join('/');

            await this.loadFileTree(parentPath);
        },

        async saveFile() {
            if (!this.currentProjectId || !this.currentFile) return;

            const response = await apiRequest(`${BASE_PATH}/api/files/${this.currentProjectId}/content?path=${encodeURIComponent(this.currentFile)}`, {
                method: 'PUT',
                body: {
                    content: this.fileContent
                }
            });

            if (response && response.ok) {
                alert('File saved successfully!');
            }
        },

        async loadGitStatus() {
            if (!this.currentProjectId) return;

            const response = await apiRequest(`${BASE_PATH}/api/git/${this.currentProjectId}/status`);
            if (response && response.ok) {
                this.gitStatus = await response.json();
            }

            const logResponse = await apiRequest(`${BASE_PATH}/api/git/${this.currentProjectId}/log?limit=10`);
            if (logResponse && logResponse.ok) {
                this.gitLog = await logResponse.json();
            }
        },

        async quickCommit() {
            if (!this.currentProjectId || !this.quickCommitMessage) return;

            this.quickCommitLoading = true;
            this.quickCommitResult = '';
            this.quickCommitSuccess = false;

            try {
                const response = await apiRequest(
                    `${BASE_PATH}/api/git/${this.currentProjectId}/quick-commit`,
                    {
                        method: 'POST',
                        headers: {
                            'Content-Type': 'application/json',
                        },
                        body: JSON.stringify({
                            message: this.quickCommitMessage
                        })
                    }
                );

                if (response && response.ok) {
                    const result = await response.json();
                    this.quickCommitSuccess = true;
                    this.quickCommitResult = `✅ ${result.message}`;
                    this.quickCommitMessage = '';

                    // Reload git status and log
                    await this.loadGitStatus();
                } else {
                    const error = await response.json();
                    this.quickCommitSuccess = false;
                    this.quickCommitResult = `❌ Error: ${error.detail || 'Failed to commit'}`;
                }
            } catch (error) {
                this.quickCommitSuccess = false;
                this.quickCommitResult = `❌ Error: ${error.message}`;
            } finally {
                this.quickCommitLoading = false;

                // Clear result after 5 seconds
                setTimeout(() => {
                    this.quickCommitResult = '';
                }, 5000);
            }
        },

        initTerminal() {
            console.log('Initializing terminal for project:', this.currentProjectId);

            if (!this.currentProjectId) {
                console.error('No project selected');
                return;
            }

            // Check if xterm is loaded
            if (typeof Terminal === 'undefined') {
                console.error('Xterm.js not loaded');
                return;
            }

            // If terminal already exists for this project, just show it
            if (this.terminals[this.currentProjectId]) {
                this.showTerminalForProject(this.currentProjectId);
                return;
            }

            // Create new terminal instance for this project
            const terminal = new Terminal({
                cursorBlink: true,
                fontSize: 14,
                fontFamily: 'Monaco, Menlo, "Ubuntu Mono", Consolas, "source-code-pro", monospace',
                theme: {
                    background: '#0c0c0c',      // Dark background like your terminal
                    foreground: '#cccccc',      // Light gray text
                    cursor: '#ffffff',
                    selection: '#264f78',
                    black: '#0c0c0c',
                    red: '#cd3131',             // Red for errors/modified files
                    green: '#0dbc79',           // Green for branch/success
                    yellow: '#e5e510',
                    blue: '#2472c8',
                    magenta: '#bc3fbc',
                    cyan: '#11a8cd',            // Cyan for paths
                    white: '#e5e5e5',
                    brightBlack: '#666666',
                    brightRed: '#f14c4c',
                    brightGreen: '#23d18b',
                    brightYellow: '#f5f543',
                    brightBlue: '#3b8eea',
                    brightMagenta: '#d670d6',
                    brightCyan: '#29b8db',
                    brightWhite: '#e5e5e5'
                }
            });

            // Create fit addon
            const fitAddon = new FitAddon.FitAddon();
            terminal.loadAddon(fitAddon);

            // Open terminal in the container
            const terminalElement = document.getElementById('terminal');
            if (terminalElement) {
                // Clear the container
                terminalElement.innerHTML = '';

                terminal.open(terminalElement);

                // Fit to container size
                setTimeout(() => {
                    fitAddon.fit();
                }, 100);

                // Store terminal info
                this.terminals[this.currentProjectId] = {
                    terminal: terminal,
                    fitAddon: fitAddon,
                    ws: null
                };

                this.currentTerminalProjectId = this.currentProjectId;

                // Connect to WebSocket
                this.connectTerminalWebSocket();
            }
        },

        showTerminalForProject(projectId) {
            console.log('Showing terminal for project:', projectId);

            const terminalData = this.terminals[projectId];
            if (!terminalData) return;

            const terminalElement = document.getElementById('terminal');
            if (terminalElement) {
                // Clear the container
                terminalElement.innerHTML = '';

                // Re-open the existing terminal
                terminalData.terminal.open(terminalElement);

                // Fit to container size
                setTimeout(() => {
                    terminalData.fitAddon.fit();
                }, 100);

                this.currentTerminalProjectId = projectId;
            }
        },

        connectTerminalWebSocket() {
            if (!this.currentProjectId) return;

            const terminalData = this.terminals[this.currentProjectId];
            if (!terminalData) return;

            // Close existing connection for this terminal
            if (terminalData.ws) {
                terminalData.ws.close();
            }

            const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
            const wsUrl = `${protocol}//${window.location.host}${BASE_PATH}/api/terminal/ws/${this.currentProjectId}`;

            const ws = new WebSocket(wsUrl);
            terminalData.ws = ws;

            ws.onopen = () => {
                console.log('Terminal WebSocket connected for project:', this.currentProjectId);

                // Send input from terminal to backend
                terminalData.terminal.onData((data) => {
                    if (ws && ws.readyState === WebSocket.OPEN) {
                        ws.send(JSON.stringify({
                            type: 'input',
                            data: data
                        }));
                    }
                });
            };

            ws.onmessage = (event) => {
                const message = JSON.parse(event.data);
                if (message.type === 'output') {
                    terminalData.terminal.write(message.data);
                }
            };

            ws.onerror = (error) => {
                console.error('Terminal WebSocket error:', error);
                terminalData.terminal.write('\r\n\x1b[31mWebSocket connection error\x1b[0m\r\n');
            };

            ws.onclose = () => {
                console.log('Terminal WebSocket closed for project:', this.currentProjectId);
                terminalData.terminal.write('\r\n\x1b[33mConnection closed\x1b[0m\r\n');
            };
        },

        async changePassword() {
            this.passwordError = '';
            this.passwordSuccess = '';

            // Validate passwords match
            if (this.passwordForm.new !== this.passwordForm.confirm) {
                this.passwordError = 'New passwords do not match';
                return;
            }

            // Validate password length
            if (this.passwordForm.new.length < 6) {
                this.passwordError = 'New password must be at least 6 characters';
                return;
            }

            try {
                const response = await apiRequest(`${BASE_PATH}/api/auth/change-password`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        current_password: this.passwordForm.current,
                        new_password: this.passwordForm.new
                    })
                });

                if (response && response.ok) {
                    this.passwordSuccess = 'Password changed successfully!';
                    this.passwordForm.current = '';
                    this.passwordForm.new = '';
                    this.passwordForm.confirm = '';
                    // Clear success message after 5 seconds
                    setTimeout(() => { this.passwordSuccess = ''; }, 5000);
                } else {
                    const error = await response.json();
                    this.passwordError = error.detail || 'Failed to change password';
                }
            } catch (error) {
                this.passwordError = 'An error occurred. Please try again.';
            }
        },

        logout() {
            localStorage.removeItem('token');
            clearCache(); // Clear all caches on logout
            window.location.href = BASE_PATH + '/login';
        },

        initMobileHandlers() {
            // Always set up mobile handlers (responsive design)
            document.addEventListener('touchstart', function() {}, {passive: true});

            // Prevent zoom on double tap for all interactive elements
            document.addEventListener('touchend', function(e) {
                const now = new Date().getTime();
                const timeSince = now - this.lastTouchEnd;
                if ((timeSince < 300) && (timeSince > 0)) {
                    e.preventDefault();
                }
                this.lastTouchEnd = now;
            }, false);

            // Additional zoom prevention
            document.addEventListener('gesturestart', function(e) {
                e.preventDefault();
            });

            // Ensure all input elements have proper font size
            this.$nextTick(() => {
                this.enforceInputFontSizes();
                this.setupTextareaAutoResize();
            });

            // Re-setup on window resize
            window.addEventListener('resize', () => {
                this.$nextTick(() => {
                    this.enforceInputFontSizes();
                    this.setupTextareaAutoResize();
                });
            });
        },

        enforceInputFontSizes() {
            // Comprehensive zoom prevention for mobile devices
            if (window.innerWidth <= 768) {
                // Target all form elements that could trigger zoom
                const formElements = document.querySelectorAll(`
                    input[type="text"],
                    input[type="password"],
                    input[type="email"],
                    input[type="search"],
                    input[type="tel"],
                    input[type="url"],
                    input[type="number"],
                    input[type="date"],
                    input[type="time"],
                    input[type="datetime-local"],
                    textarea,
                    select
                `);

                formElements.forEach(element => {
                    // Apply comprehensive zoom prevention styles
                    element.style.fontSize = '16px';
                    element.style.webkitTextSizeAdjust = '100%';
                    element.style.webkitAppearance = 'none';
                    element.style.mozAppearance = 'none';
                    element.style.appearance = 'none';
                    element.style.touchAction = 'manipulation';

                    // Ensure minimum touch target size for better UX
                    if (element.tagName.toLowerCase() === 'input' || element.tagName.toLowerCase() === 'select') {
                        element.style.minHeight = '44px';
                    }
                });

                // Add additional zoom prevention event listeners
                this.addZoomPreventionListeners();
            }
        },

        addZoomPreventionListeners() {
            // Prevent double-tap zoom
            let lastTouchEnd = 0;
            document.addEventListener('touchend', function(event) {
                const now = (new Date()).getTime();
                if (now - lastTouchEnd <= 300) {
                    event.preventDefault();
                }
                lastTouchEnd = now;
            }, false);

            // Prevent gesture zoom
            document.addEventListener('gesturestart', function(event) {
                event.preventDefault();
            }, false);

            // Prevent wheel zoom on mobile
            document.addEventListener('wheel', function(event) {
                if (event.ctrlKey) {
                    event.preventDefault();
                }
            }, { passive: false });
        },

        setupTextareaAutoResize() {
            const textarea = document.querySelector('textarea[x-model="messageInput"]');
            if (textarea) {
                // Remove existing listeners to prevent duplicates
                textarea.removeEventListener('input', this.textareaResizeHandler);

                // Create bound handler
                this.textareaResizeHandler = () => {
                    // Reset height to auto to get the correct scrollHeight
                    textarea.style.height = 'auto';

                    // Calculate new height (min 44px for mobile touch targets, max 120px)
                    const minHeight = window.innerWidth <= 768 ? 44 : 40;
                    const maxHeight = 120;
                    const newHeight = Math.max(minHeight, Math.min(textarea.scrollHeight, maxHeight));

                    textarea.style.height = newHeight + 'px';
                };

                // Set initial height
                this.textareaResizeHandler();

                // Add input event listener for auto-resize
                textarea.addEventListener('input', this.textareaResizeHandler);

                // Also resize on paste
                textarea.addEventListener('paste', () => {
                    setTimeout(this.textareaResizeHandler, 0);
                });
            }
        }
    };
}

// Auto-scan projects when modal opens
document.addEventListener('alpine:init', () => {
    Alpine.watch('showProjectScan', (value) => {
        if (value) {
            const app = Alpine.$data(document.querySelector('[x-data]'));
            app.scanProjects();
        }
    });
});

