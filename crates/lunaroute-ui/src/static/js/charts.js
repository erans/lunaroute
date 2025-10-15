// LunaRoute UI - Chart Initialization
// Uses Chart.js loaded from CDN

let tokenChart = null;
let toolChart = null;
let hourOfDayChart = null;
let callsPerModelChart = null;
let spendingByModelChart = null;

/**
 * Initialize token usage chart
 */
async function initTokenChart() {
    const ctx = document.getElementById('tokenChart');
    if (!ctx) return;

    // Get days from current time range (convert hours to days)
    const hours = typeof currentTimeRangeHours !== 'undefined' ? currentTimeRangeHours : 24;
    const days = Math.max(1, Math.ceil(hours / 24));

    // Fetch initial data
    const response = await fetch(`/api/stats/tokens?days=${days}`);
    const data = await response.json();

    tokenChart = new Chart(ctx, {
        type: 'line',
        data: {
            labels: data.map(d => d.date),
            datasets: [
                {
                    label: 'Input Tokens',
                    data: data.map(d => d.input_tokens),
                    borderColor: '#3b82f6',
                    backgroundColor: 'rgba(59, 130, 246, 0.1)',
                    tension: 0.4
                },
                {
                    label: 'Output Tokens',
                    data: data.map(d => d.output_tokens),
                    borderColor: '#10b981',
                    backgroundColor: 'rgba(16, 185, 129, 0.1)',
                    tension: 0.4
                },
                {
                    label: 'Thinking Tokens',
                    data: data.map(d => d.thinking_tokens),
                    borderColor: '#8b5cf6',
                    backgroundColor: 'rgba(139, 92, 246, 0.1)',
                    tension: 0.4
                }
            ]
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: {
                legend: {
                    labels: {
                        color: '#f1f5f9'
                    }
                }
            },
            scales: {
                y: {
                    beginAtZero: true,
                    ticks: {
                        color: '#cbd5e1'
                    },
                    grid: {
                        color: '#334155'
                    }
                },
                x: {
                    ticks: {
                        color: '#cbd5e1'
                    },
                    grid: {
                        color: '#334155'
                    }
                }
            }
        }
    });

    // Auto-update chart
    setInterval(async () => {
        const hours = typeof currentTimeRangeHours !== 'undefined' ? currentTimeRangeHours : 24;
        const days = Math.max(1, Math.ceil(hours / 24));

        const response = await fetch(`/api/stats/tokens?days=${days}`);
        const data = await response.json();

        tokenChart.data.labels = data.map(d => d.date);
        tokenChart.data.datasets[0].data = data.map(d => d.input_tokens);
        tokenChart.data.datasets[1].data = data.map(d => d.output_tokens);
        tokenChart.data.datasets[2].data = data.map(d => d.thinking_tokens);
        tokenChart.update('none'); // No animation for updates
    }, 5000);
}

/**
 * Update token chart with new time range
 */
async function updateTokenChart() {
    if (!tokenChart) return;

    const hours = typeof currentTimeRangeHours !== 'undefined' ? currentTimeRangeHours : 24;
    const days = Math.max(1, Math.ceil(hours / 24));

    const response = await fetch(`/api/stats/tokens?days=${days}`);
    const data = await response.json();

    tokenChart.data.labels = data.map(d => d.date);
    tokenChart.data.datasets[0].data = data.map(d => d.input_tokens);
    tokenChart.data.datasets[1].data = data.map(d => d.output_tokens);
    tokenChart.data.datasets[2].data = data.map(d => d.thinking_tokens);
    tokenChart.update();
}

/**
 * Initialize tool usage chart with failure tracking
 */
async function initToolChart() {
    const ctx = document.getElementById('toolChart');
    if (!ctx) return;

    // Fetch initial data
    const response = await fetch('/api/stats/tools');
    const data = await response.json();

    // Sort by call count
    data.sort((a, b) => b.call_count - a.call_count);

    // Take top 10
    const top10 = data.slice(0, 10);

    toolChart = new Chart(ctx, {
        type: 'bar',
        data: {
            labels: top10.map(t => t.tool_name),
            datasets: [{
                label: 'Call Count',
                data: top10.map(t => t.call_count),
                backgroundColor: top10.map(t => {
                    // Color by success rate
                    const rate = t.success_rate || 100;
                    if (rate >= 95) return '#10b981'; // Excellent (≥95%) - green
                    if (rate >= 80) return '#f59e0b'; // Warning (80-95%) - yellow
                    return '#ef4444'; // Critical (<80%) - red
                })
            }]
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            indexAxis: 'y',
            plugins: {
                legend: {
                    display: false
                },
                tooltip: {
                    callbacks: {
                        label: function(context) {
                            const tool = top10[context.dataIndex];
                            const rate = tool.success_rate || 100;

                            // Determine status indicator
                            let indicator = '✓'; // Success
                            if (rate < 95 && rate >= 80) indicator = '⚠'; // Warning
                            if (rate < 80) indicator = '✗'; // Critical

                            return [
                                `${indicator} Success Rate: ${rate.toFixed(1)}%`,
                                `Total Calls: ${tool.call_count}`,
                                `Successes: ${tool.success_count}`,
                                `Failures: ${tool.failure_count}`,
                                `Avg Time: ${tool.avg_time_ms.toFixed(1)}ms`
                            ];
                        }
                    }
                }
            },
            scales: {
                x: {
                    beginAtZero: true,
                    ticks: {
                        color: '#cbd5e1'
                    },
                    grid: {
                        color: '#334155'
                    }
                },
                y: {
                    ticks: {
                        color: '#cbd5e1'
                    },
                    grid: {
                        color: '#334155'
                    }
                }
            }
        }
    });

    // Auto-update chart
    setInterval(async () => {
        const response = await fetch('/api/stats/tools');
        const data = await response.json();
        data.sort((a, b) => b.call_count - a.call_count);
        const top10 = data.slice(0, 10);

        toolChart.data.labels = top10.map(t => t.tool_name);
        toolChart.data.datasets[0].data = top10.map(t => t.call_count);
        toolChart.data.datasets[0].backgroundColor = top10.map(t => {
            const rate = t.success_rate || 100;
            if (rate >= 95) return '#10b981';
            if (rate >= 80) return '#f59e0b';
            return '#ef4444';
        });
        toolChart.update('none');
    }, 5000);
}

/**
 * Initialize hour of day chart
 */
async function initHourOfDayChart() {
    const ctx = document.getElementById('hourOfDayChart');
    if (!ctx) return;

    const hours = typeof currentTimeRangeHours !== 'undefined' ? currentTimeRangeHours : 24;

    // Fetch initial data
    const response = await fetch(`/api/stats/hours?hours=${hours}`);
    const data = await response.json();

    // Create array of all 24 hours with session counts
    const hourData = Array(24).fill(0);
    data.forEach(d => {
        if (d.hour >= 0 && d.hour < 24) {
            hourData[d.hour] = d.session_count;
        }
    });

    const labels = Array.from({length: 24}, (_, i) => `${i}:00`);

    hourOfDayChart = new Chart(ctx, {
        type: 'bar',
        data: {
            labels: labels,
            datasets: [{
                label: 'Sessions',
                data: hourData,
                backgroundColor: '#3b82f6',
                borderColor: '#2563eb',
                borderWidth: 1
            }]
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: {
                legend: {
                    display: false
                }
            },
            scales: {
                y: {
                    beginAtZero: true,
                    ticks: {
                        color: '#cbd5e1',
                        stepSize: 1
                    },
                    grid: {
                        color: '#334155'
                    }
                },
                x: {
                    ticks: {
                        color: '#cbd5e1',
                        maxRotation: 45,
                        minRotation: 45
                    },
                    grid: {
                        color: '#334155'
                    }
                }
            }
        }
    });

    // Auto-update chart
    setInterval(async () => {
        const hours = typeof currentTimeRangeHours !== 'undefined' ? currentTimeRangeHours : 24;
        const response = await fetch(`/api/stats/hours?hours=${hours}`);
        const data = await response.json();

        const hourData = Array(24).fill(0);
        data.forEach(d => {
            if (d.hour >= 0 && d.hour < 24) {
                hourData[d.hour] = d.session_count;
            }
        });

        hourOfDayChart.data.datasets[0].data = hourData;
        hourOfDayChart.update('none');
    }, 5000);
}

/**
 * Update hour of day chart with new time range
 */
async function updateHourOfDayChart() {
    if (!hourOfDayChart) return;

    const hours = typeof currentTimeRangeHours !== 'undefined' ? currentTimeRangeHours : 24;
    const response = await fetch(`/api/stats/hours?hours=${hours}`);
    const data = await response.json();

    const hourData = Array(24).fill(0);
    data.forEach(d => {
        if (d.hour >= 0 && d.hour < 24) {
            hourData[d.hour] = d.session_count;
        }
    });

    hourOfDayChart.data.datasets[0].data = hourData;
    hourOfDayChart.update();
}

/**
 * Initialize calls per model chart
 */
async function initCallsPerModelChart() {
    const ctx = document.getElementById('callsPerModelChart');
    if (!ctx) return;

    // NO hours parameter - we want all-time cumulative data
    const response = await fetch('/api/stats/spending');
    const data = await response.json();

    // Take top 10 models by call count (session count)
    const sorted = [...data.by_model].sort((a, b) => b.session_count - a.session_count);
    const top10 = sorted.slice(0, 10);

    callsPerModelChart = new Chart(ctx, {
        type: 'bar',
        data: {
            labels: top10.map(m => m.model_name),
            datasets: [{
                label: 'Total Calls',
                data: top10.map(m => m.session_count),
                backgroundColor: '#3b82f6',
                borderColor: '#2563eb',
                borderWidth: 1
            }]
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            indexAxis: 'y',
            plugins: {
                legend: {
                    display: false
                },
                tooltip: {
                    callbacks: {
                        label: function(context) {
                            const model = top10[context.dataIndex];
                            return [
                                `Total Calls: ${model.session_count}`,
                                `Total Cost: $${model.total_cost.toFixed(4)}`,
                                `Avg Cost/Call: $${model.avg_cost_per_session.toFixed(4)}`
                            ];
                        }
                    }
                }
            },
            scales: {
                x: {
                    beginAtZero: true,
                    ticks: {
                        color: '#cbd5e1',
                        stepSize: 1
                    },
                    grid: {
                        color: '#334155'
                    }
                },
                y: {
                    ticks: {
                        color: '#cbd5e1'
                    },
                    grid: {
                        color: '#334155'
                    }
                }
            }
        }
    });

    // Auto-update chart (no hours parameter - all-time data)
    setInterval(async () => {
        const response = await fetch('/api/stats/spending');
        const data = await response.json();
        const sorted = [...data.by_model].sort((a, b) => b.session_count - a.session_count);
        const top10 = sorted.slice(0, 10);

        callsPerModelChart.data.labels = top10.map(m => m.model_name);
        callsPerModelChart.data.datasets[0].data = top10.map(m => m.session_count);
        callsPerModelChart.update('none');
    }, 5000);
}

/**
 * Update calls per model chart with new time range
 */
async function updateCallsPerModelChart() {
    if (!callsPerModelChart) return;

    // NO hours parameter - we want all-time cumulative data
    // This chart should NOT be affected by the time range selector
    const response = await fetch('/api/stats/spending');
    const data = await response.json();
    const sorted = [...data.by_model].sort((a, b) => b.session_count - a.session_count);
    const top10 = sorted.slice(0, 10);

    callsPerModelChart.data.labels = top10.map(m => m.model_name);
    callsPerModelChart.data.datasets[0].data = top10.map(m => m.session_count);
    callsPerModelChart.update();
}


/**
 * Initialize spending by model chart
 */
async function initSpendingByModelChart() {
    const ctx = document.getElementById('spendingByModelChart');
    if (!ctx) return;

    const hours = typeof currentTimeRangeHours !== 'undefined' ? currentTimeRangeHours : 24;

    // Fetch initial data
    const response = await fetch(`/api/stats/spending?hours=${hours}`);
    const data = await response.json();

    // Take top 10 models by cost
    const top10 = data.by_model.slice(0, 10);

    spendingByModelChart = new Chart(ctx, {
        type: 'bar',
        data: {
            labels: top10.map(m => m.model_name),
            datasets: [{
                label: 'Total Cost ($)',
                data: top10.map(m => m.total_cost),
                backgroundColor: '#10b981',
                borderColor: '#059669',
                borderWidth: 1
            }]
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            indexAxis: 'y',
            plugins: {
                legend: {
                    display: false
                },
                tooltip: {
                    callbacks: {
                        label: function(context) {
                            const model = top10[context.dataIndex];
                            return [
                                `Total: $${model.total_cost.toFixed(4)}`,
                                `Sessions: ${model.session_count}`,
                                `Avg/session: $${model.avg_cost_per_session.toFixed(4)}`
                            ];
                        }
                    }
                }
            },
            scales: {
                x: {
                    beginAtZero: true,
                    ticks: {
                        color: '#cbd5e1',
                        callback: function(value) {
                            return '$' + value.toFixed(4);
                        }
                    },
                    grid: {
                        color: '#334155'
                    }
                },
                y: {
                    ticks: {
                        color: '#cbd5e1'
                    },
                    grid: {
                        color: '#334155'
                    }
                }
            }
        }
    });

    // Auto-update chart
    setInterval(async () => {
        const hours = typeof currentTimeRangeHours !== 'undefined' ? currentTimeRangeHours : 24;
        const response = await fetch(`/api/stats/spending?hours=${hours}`);
        const data = await response.json();
        const top10 = data.by_model.slice(0, 10);

        spendingByModelChart.data.labels = top10.map(m => m.model_name);
        spendingByModelChart.data.datasets[0].data = top10.map(m => m.total_cost);
        spendingByModelChart.update('none');
    }, 5000);
}

/**
 * Update spending chart with new time range
 */
async function updateSpendingByModelChart() {
    if (!spendingByModelChart) return;

    const hours = typeof currentTimeRangeHours !== 'undefined' ? currentTimeRangeHours : 24;
    const response = await fetch(`/api/stats/spending?hours=${hours}`);
    const data = await response.json();
    const top10 = data.by_model.slice(0, 10);

    spendingByModelChart.data.labels = top10.map(m => m.model_name);
    spendingByModelChart.data.datasets[0].data = top10.map(m => m.total_cost);
    spendingByModelChart.update();
}
