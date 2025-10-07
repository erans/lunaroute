// LunaRoute UI - Main Application Logic
// Embedded in binary, no external dependencies

/**
 * Dashboard auto-refresh class
 * Handles periodic fetching of stats with smart pausing
 */
class DashboardRefresher {
    constructor(interval = 5000) {
        this.interval = interval;
        this.timerId = null;
        this.isVisible = true;

        // Stop refreshing when tab is hidden (save resources)
        document.addEventListener('visibilitychange', () => {
            this.isVisible = !document.hidden;
            if (this.isVisible) {
                this.refresh(); // Immediate refresh when returning
            }
        });
    }

    start() {
        this.refresh(); // Initial load
        this.timerId = setInterval(() => {
            if (this.isVisible) {
                this.refresh();
            }
        }, this.interval);
    }

    stop() {
        if (this.timerId) {
            clearInterval(this.timerId);
        }
    }

    async refresh() {
        try {
            // Get time range from global variable (set by dashboard)
            const hours = typeof currentTimeRangeHours !== 'undefined' ? currentTimeRangeHours : 24;

            const [stats, costs, recentSessions] = await Promise.all([
                fetch(`/api/stats/overview?hours=${hours}`).then(r => r.json()),
                fetch('/api/stats/costs').then(r => r.json()),
                fetch('/api/sessions/recent').then(r => r.json()),
            ]);

            // Update overview cards
            updateOverviewCards(stats);

            // Update cost cards
            updateCostCards(costs);

            // Update recent sessions table
            updateRecentSessions(recentSessions);

            // Update last updated time
            updateLastUpdated();

        } catch (error) {
            console.error('Refresh failed:', error);
            // Continue trying - don't stop on error
        }
    }
}

/**
 * Update overview statistics cards
 */
function updateOverviewCards(stats) {
    document.getElementById('stat-sessions').textContent = formatNumber(stats.total_sessions);
    document.getElementById('stat-tokens').textContent = formatNumber(stats.total_tokens);
    document.getElementById('stat-cost').textContent = '$' + stats.total_cost.toFixed(4);
    document.getElementById('stat-success').textContent = stats.success_rate.toFixed(1) + '%';
}

/**
 * Update cost trend cards
 */
function updateCostCards(costs) {
    document.getElementById('cost-today').textContent = '$' + costs.today.toFixed(4);
    document.getElementById('cost-week').textContent = '$' + costs.this_week.toFixed(4);
    document.getElementById('cost-month').textContent = '$' + costs.this_month.toFixed(4);
    document.getElementById('cost-projection').textContent = '$' + costs.projection_monthly.toFixed(2);
}

/**
 * Update recent sessions table
 */
function updateRecentSessions(sessions) {
    const tbody = document.getElementById('sessions-tbody');

    if (sessions.length === 0) {
        tbody.innerHTML = '<tr><td colspan="6" class="loading">No sessions found</td></tr>';
        return;
    }

    tbody.innerHTML = sessions.map(session => `
        <tr onclick="window.location='/sessions/${session.session_id}'">
            <td>${formatTimeAgo(session.started_at)}</td>
            <td>${session.model}</td>
            <td>${session.request_count}</td>
            <td>${formatNumber(session.total_tokens)}</td>
            <td>$${session.cost.toFixed(4)}</td>
            <td>${formatDuration(session.duration_ms)}</td>
        </tr>
    `).join('');
}

/**
 * Render sessions table (for sessions list page)
 */
function renderSessionsTable(sessions, tbodyId) {
    const tbody = document.getElementById(tbodyId);

    if (sessions.length === 0) {
        tbody.innerHTML = '<tr><td colspan="7" class="loading">No sessions found</td></tr>';
        return;
    }

    tbody.innerHTML = sessions.map(session => `
        <tr onclick="window.location='/sessions/${session.session_id}'">
            <td>${formatTimeAgo(session.started_at)}</td>
            <td>${session.model}</td>
            <td>${session.request_count}</td>
            <td>${formatNumber(session.total_tokens)}</td>
            <td>$${session.cost.toFixed(4)}</td>
            <td>${formatDuration(session.duration_ms)}</td>
            <td><a href="/sessions/${session.session_id}" onclick="event.stopPropagation()">View</a></td>
        </tr>
    `).join('');
}

/**
 * Update last updated timestamp
 */
function updateLastUpdated() {
    const now = new Date();
    const timeStr = now.toLocaleTimeString();
    document.getElementById('last-updated').textContent = `Last updated: ${timeStr}`;
}

/**
 * Format number with commas
 */
function formatNumber(num) {
    return num.toLocaleString();
}

/**
 * Format duration in milliseconds to human-readable
 */
function formatDuration(ms) {
    if (ms < 1000) {
        return ms + 'ms';
    } else if (ms < 60000) {
        return (ms / 1000).toFixed(1) + 's';
    } else {
        return (ms / 60000).toFixed(1) + 'm';
    }
}

/**
 * Format timestamp to relative time (e.g., "2m ago")
 */
function formatTimeAgo(timestamp) {
    const now = new Date();
    const past = new Date(timestamp);
    const diffMs = now - past;
    const diffSec = Math.floor(diffMs / 1000);
    const diffMin = Math.floor(diffSec / 60);
    const diffHour = Math.floor(diffMin / 60);
    const diffDay = Math.floor(diffHour / 24);

    if (diffSec < 60) {
        return diffSec + 's ago';
    } else if (diffMin < 60) {
        return diffMin + 'm ago';
    } else if (diffHour < 24) {
        return diffHour + 'h ago';
    } else {
        return diffDay + 'd ago';
    }
}

/**
 * Set active nav link based on current page
 */
function setActiveNavLink() {
    const path = window.location.pathname;
    const links = document.querySelectorAll('.nav-link');

    links.forEach(link => {
        const href = link.getAttribute('href');
        if (path === href || (path.startsWith(href) && href !== '/')) {
            link.style.color = 'var(--color-primary)';
            link.style.fontWeight = '600';
        }
    });
}

// Set active nav link on page load
document.addEventListener('DOMContentLoaded', setActiveNavLink);
