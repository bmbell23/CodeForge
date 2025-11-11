// Base path for API calls (empty for development, '/code' for production)
const BASE_PATH = window.location.pathname.startsWith('/code/') ? '/code' : '';

function dashboardApp() {
    return {
        // User data
        username: '',

        // Projects
        projects: [],
        currentProjectId: '',
        scannedProjects: [],
        showProjectScan: false,
        
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
        
        // Files
        fileTree: [],
        currentFile: null,
        fileContent: '',
        currentPath: '', // Track current directory path
        loadingFileTree: false,
        
        // Git
        gitStatus: null,
        gitLog: [],

        // Terminal - store per project
        terminals: {}, // Map of project_id -> {terminal, fitAddon, ws}
        currentTerminalProjectId: null,

        async init() {
            await this.loadUser();
            await this.loadProjects();
            this.configureMarked();
            this.initMobileHandlers();

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

        renderMarkdown(content) {
            if (!content) return '';

            // Strip ANSI codes first
            content = this.stripAnsiCodes(content);

            if (typeof marked === 'undefined') {
                // Fallback if marked.js isn't loaded
                return content.replace(/\n/g, '<br>');
            }
            try {
                return marked.parse(content);
            } catch (err) {
                console.error('Markdown rendering error:', err);
                return content.replace(/\n/g, '<br>');
            }
        },
        
        async loadUser() {
            const response = await apiRequest(`${BASE_PATH}/api/auth/me`);
            if (response && response.ok) {
                const user = await response.json();
                this.username = user.username;
            }
        },

        async loadProjects() {
            const response = await apiRequest(`${BASE_PATH}/api/projects/`);
            if (response && response.ok) {
                this.projects = await response.json();
                if (this.projects.length > 0 && !this.currentProjectId) {
                    this.currentProjectId = this.projects[0].id;
                    await this.switchProject();
                }
            }
        },
        
        async switchProject() {
            if (!this.currentProjectId) return;

            // Load conversations for this project
            await this.loadConversations();

            // Load file tree
            await this.loadFileTree();

            // Load git status
            await this.loadGitStatus();

            // If terminal tab is active, switch to the terminal for this project
            if (this.currentTab === 'terminal') {
                this.initTerminal();
            }
        },
        
        async loadConversations() {
            const url = this.currentProjectId
                ? `${BASE_PATH}/api/chat/conversations?project_id=${this.currentProjectId}`
                : `${BASE_PATH}/api/chat/conversations`;

            const response = await apiRequest(url);
            if (response && response.ok) {
                this.conversations = await response.json();
                
                // Select first conversation if available
                if (this.conversations.length > 0 && !this.currentConversationId) {
                    await this.selectConversation(this.conversations[0].id);
                }
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

            // Close existing WebSocket
            if (this.ws) {
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
        
        async loadMessages() {
            const response = await apiRequest(`${BASE_PATH}/api/chat/conversations/${this.currentConversationId}/messages`);
            if (response && response.ok) {
                this.messages = await response.json();
                this.$nextTick(() => this.scrollToBottom());
            }
        },

        connectWebSocket() {
            const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
            const wsUrl = `${protocol}//${window.location.host}${BASE_PATH}/api/chat/ws/${this.currentConversationId}`;

            this.ws = new WebSocket(wsUrl);
            
            this.ws.onmessage = (event) => {
                const data = JSON.parse(event.data);
                
                if (data.type === 'user_message') {
                    this.messages.push(data.message);
                    this.$nextTick(() => this.scrollToBottom());
                } else if (data.type === 'assistant_chunk') {
                    this.isStreaming = true;
                    this.streamingContent += data.chunk;
                    this.$nextTick(() => this.scrollToBottom());
                } else if (data.type === 'assistant_complete') {
                    this.isStreaming = false;
                    this.messages.push(data.message);
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
            
            this.ws.onclose = () => {
                console.log('WebSocket closed');
            };
        },
        
        sendMessage() {
            if ((!this.messageInput.trim() && this.attachedImages.length === 0) || !this.ws || this.isStreaming) return;

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
        
        scrollToBottom() {
            const container = document.getElementById('messagesContainer');
            if (container) {
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

        logout() {
            localStorage.removeItem('token');
            window.location.href = '/login';
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
            // Ensure all input elements have 16px font size to prevent zoom
            const inputs = document.querySelectorAll('input, textarea, select, button');
            inputs.forEach(input => {
                if (window.innerWidth <= 768) {
                    input.style.fontSize = '16px';
                    input.style.webkitTextSizeAdjust = '100%';
                    input.style.webkitAppearance = 'none';
                }
            });
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

