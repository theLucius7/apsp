// One settings record backs both the panel settings and the alert settings
// pages. Updates are partial: each page sends only the fields it shows and the
// panel keeps the stored value of everything else.
export type Preferences = { public_dashboard: boolean; notification_enabled: boolean; offline_alerts: boolean; offline_minutes: number; expiry_alert_days: number; traffic_alert_percentage: number; telegram_enabled: boolean; telegram_chat_id: string; telegram_token_configured: boolean; telegram_thread_id: number | null; telegram_template: string; show_frozen_features?: boolean }
