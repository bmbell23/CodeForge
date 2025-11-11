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
        
        // UI
        currentTab: 'chat',
        
        // Files
        fileTree: [],
        currentFile: null,
        fileContent: '',
        
        // Git
        gitStatus: null,
        gitLog: [],
        
        async init() {
            await this.loadUser();
            await this.loadProjects();
        },
        
        async loadUser() {
            const response = await apiRequest('/api/auth/me');
            if (response && response.ok) {
                const user = await response.json();
                this.username = user.username;
            }
        },
        
        async loadProjects() {
            const response = await apiRequest('/api/projects/');
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
        },
        
        async loadConversations() {
            const url = this.currentProjectId 
                ? `/api/chat/conversations?project_id=${this.currentProjectId}`
                : '/api/chat/conversations';
            
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
            const response = await apiRequest('/api/chat/conversations', {
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

            const response = await apiRequest(`/api/chat/conversations/${conversationId}`, {
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
            const response = await apiRequest(`/api/chat/conversations/${this.currentConversationId}/messages`);
            if (response && response.ok) {
                this.messages = await response.json();
                this.$nextTick(() => this.scrollToBottom());
            }
        },
        
        connectWebSocket() {
            const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
            const wsUrl = `${protocol}//${window.location.host}/api/chat/ws/${this.currentConversationId}`;
            
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
            if (!this.messageInput.trim() || !this.ws || this.isStreaming) return;
            
            this.ws.send(JSON.stringify({
                message: this.messageInput
            }));
            
            this.messageInput = '';
        },
        
        scrollToBottom() {
            const container = document.getElementById('messagesContainer');
            if (container) {
                container.scrollTop = container.scrollHeight;
            }
        },
        
        async scanProjects() {
            const response = await apiRequest('/api/projects/scan');
            if (response && response.ok) {
                this.scannedProjects = await response.json();
                this.showProjectScan = true;
            }
        },
        
        async addProject(project) {
            const response = await apiRequest('/api/projects/', {
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
        
        async loadFileTree() {
            if (!this.currentProjectId) return;
            
            const response = await apiRequest(`/api/files/${this.currentProjectId}/tree`);
            if (response && response.ok) {
                this.fileTree = await response.json();
            }
        },
        
        async openFile(path) {
            if (!this.currentProjectId) return;
            
            const response = await apiRequest(`/api/files/${this.currentProjectId}/content?path=${encodeURIComponent(path)}`);
            if (response && response.ok) {
                const data = await response.json();
                this.currentFile = path;
                this.fileContent = data.content;
            }
        },
        
        async saveFile() {
            if (!this.currentProjectId || !this.currentFile) return;
            
            const response = await apiRequest(`/api/files/${this.currentProjectId}/content?path=${encodeURIComponent(this.currentFile)}`, {
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
            
            const response = await apiRequest(`/api/git/${this.currentProjectId}/status`);
            if (response && response.ok) {
                this.gitStatus = await response.json();
            }
            
            const logResponse = await apiRequest(`/api/git/${this.currentProjectId}/log?limit=10`);
            if (logResponse && logResponse.ok) {
                this.gitLog = await logResponse.json();
            }
        },
        
        logout() {
            localStorage.removeItem('token');
            window.location.href = '/login';
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

