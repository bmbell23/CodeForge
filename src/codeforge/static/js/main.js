// Main JavaScript utilities

// API helper function
async function apiRequest(url, options = {}) {
    const token = localStorage.getItem('token');

    const headers = {
        ...options.headers,
    };

    if (token) {
        headers['Authorization'] = `Bearer ${token}`;
    }

    if (options.body && typeof options.body === 'object' && !(options.body instanceof FormData)) {
        headers['Content-Type'] = 'application/json';
        options.body = JSON.stringify(options.body);
    }

    const response = await fetch(url, {
        ...options,
        headers
    });

    if (response.status === 401) {
        // Unauthorized - redirect to login
        localStorage.removeItem('token');
        window.location.href = '/login';
        return null;
    }

    return response;
}

// Check if user is authenticated
async function checkAuth() {
    const token = localStorage.getItem('token');
    const isLoginPage = window.location.pathname.includes('/login') || window.location.pathname.includes('/register');

    // If no token and not on login page, redirect to login
    if (!token && !isLoginPage) {
        window.location.href = '/login';
        return;
    }

    // If we have a token and not on login page, verify it's valid
    if (token && !isLoginPage) {
        try {
            const BASE_PATH = window.location.pathname.startsWith('/code/') ? '/code' : '';
            const response = await fetch(`${BASE_PATH}/api/auth/me`, {
                headers: {
                    'Authorization': `Bearer ${token}`
                }
            });

            if (!response.ok) {
                // Token is invalid, clear it and redirect to login
                localStorage.removeItem('token');
                window.location.href = `${BASE_PATH}/login`;
                return;
            }
        } catch (error) {
            console.error('Auth check failed:', error);
            // On error, clear token and redirect to login
            localStorage.removeItem('token');
            window.location.href = '/login';
        }
    }
}

// Run auth check on page load
document.addEventListener('DOMContentLoaded', checkAuth);

