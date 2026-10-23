// One settings record backs both the panel settings and the alert settings
// pages. The update endpoint requires the full set of switches, so each page
// re-reads the latest record and changes only the fields it shows.
export type Preferences = { public_dashboard: boolean; notification_enabled: boolean; offline_alerts: boolean; offline_minutes: number; expiry_alert_days: number; traffic_alert_percentage: number; telegram_enabled: boolean; telegram_chat_id: string; telegram_token_configured: boolean; telegram_thread_id: number | null; telegram_template: string }
