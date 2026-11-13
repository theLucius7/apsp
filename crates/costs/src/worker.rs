use super::*;
use apsp_panel_host::{
    maintenance::{self, Cadence},
    notifications::{
        Failure, delivery,
        queue::{self, Backoff, Claim, Claimed, Claiming, Claims, Outcome, Policy, Queue},
        webhook::Message,
    },
    ports::BoxFuture,
    settings,
};
use axum::{Json, extract::State};
use time::Duration;

/// Renewal reminders: leased one at a time under the expenses lock and sent after the claim
/// commits, up to twenty per pass, five sends each.
const POLICY: Policy = Policy {
    table: "expense_deliveries",
    claims: 20,
    claiming: Claiming::Leased {
        seconds: 120,
        batch: 1,
    },
    attempts: 5,
    backoff: Backoff::Doubling {
        first: 30,
        doublings: 4,
        min: 30,
        max: 86_400,
    },
    held: None,
};

pub(super) async fn run(state: AppState) {
    maintenance::periodic(
        "expenses",
        state.pool.clone(),
        Cadence::every(std::time::Duration::from_secs(60)),
        state,
        |state| {
            Box::pin(async move {
                maintenance::outcome(tick(state).await, json!({"source":"renewal-cycle"}))
            })
        },
    )
    .await
}

pub async fn tick(state: &AppState) -> ApiResult<()> {
    let settings = settings::read(&state.pool).await?;
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    let config=sqlx::query("SELECT reminders_enabled,(to_timestamp($1) AT TIME ZONE timezone)::date::text AS today FROM expense_settings WHERE singleton").bind(now_timestamp()).fetch_one(&mut *tx).await?;
    let reminders: bool = config.try_get("reminders_enabled")?;
    let today = schedule::parse(&config.try_get::<String, _>("today")?)?;
    let rows:Vec<Subscription>=sqlx::query_as(&format!("SELECT {COLUMNS} FROM expense_subscriptions WHERE NOT inactive ORDER BY id LIMIT 10000 FOR UPDATE")).fetch_all(&mut *tx).await?;
    for mut entry in rows {
        let next = schedule::parse(&entry.next_payment)?;
        let cancel = entry
            .cancellation_date
            .as_deref()
            .map(schedule::parse)
            .transpose()?;
        if cancel.is_some_and(|d| d < today) {
            sqlx::query("UPDATE expense_subscriptions SET inactive=TRUE,version=version+1,updated_at=$2 WHERE id=$1").bind(entry.id).bind(now_timestamp()).execute(&mut *tx).await?;
            event(
                &mut tx,
                Some(entry.id),
                "cancelled_on_date",
                json!({"date":schedule::format(today)}),
            )
            .await?;
            continue;
        }
        if entry.auto_renew && next < today {
            let Some((next, _)) = schedule::at_or_after(
                schedule::parse(&entry.anchor_payment)?,
                &entry.cycle,
                entry.frequency,
                today,
            ) else {
                continue;
            };
            if cancel.is_some_and(|d| d < next) {
                continue;
            }
            entry.next_payment = schedule::format(next);
            sqlx::query("UPDATE expense_subscriptions SET next_payment=$2::text::date,version=version+1,updated_at=$3 WHERE id=$1").bind(entry.id).bind(&entry.next_payment).bind(now_timestamp()).execute(&mut *tx).await?;
            entry.version += 1;
            event(
                &mut tx,
                Some(entry.id),
                "schedule_advanced",
                json!({"next_payment":entry.next_payment,"payment_confirmed":false}),
            )
            .await?;
        }
        if !reminders || !entry.notify {
            continue;
        }
        let next = schedule::parse(&entry.next_payment)?;
        if next < today
            || cancel.is_some_and(|d| d < next)
            || next - today > Duration::days(i64::from(entry.notify_days_before))
        {
            continue;
        }
        // One reminder per record and due date; editing the record inside the
        // reminder window must not send it again.
        let key = format!("due:{}:{}", entry.id, entry.next_payment);
        let id = Uuid::new_v4();
        let timestamp = now_timestamp();
        let amount = units(&entry.amount_units)?;
        let message = format!(
            "{} 将于 {} 续费，金额 {} {}.{:04}。{}",
            entry.name,
            entry.next_payment,
            entry.currency,
            amount / 10000,
            amount % 10000,
            if entry.auto_renew {
                "已标记为自动续费，请核对服务商账单。"
            } else {
                "请手动确认是否续费。"
            }
        );
        let id_text = id.to_string();
        let time_text = timestamp.to_string();
        let payloads = delivery::render(
            &settings,
            &Message {
                title: "订阅续费提醒",
                server: "",
                message: &message,
                time: &time_text,
                event: "expense_due",
                event_id: &id_text,
                category: "expenses",
            },
        );
        if payloads.is_empty() {
            continue;
        }
        // Reminders recorded under the earlier versioned key also count for this due date.
        let added=sqlx::query("INSERT INTO expense_events(id,subscription_id,event_key,kind,detail,created_at) SELECT $1,$2,$3,'renewal_reminder',$4,$5 WHERE NOT EXISTS(SELECT 1 FROM expense_events WHERE subscription_id=$2 AND kind='renewal_reminder' AND detail->>'due'=$6) ON CONFLICT(event_key) DO NOTHING").bind(id).bind(entry.id).bind(key).bind(json!({"name":entry.name,"due":entry.next_payment,"amount_units":entry.amount_units,"currency":entry.currency})).bind(timestamp).bind(&entry.next_payment).execute(&mut *tx).await?.rows_affected();
        if added == 0 {
            continue;
        }
        for (channel, payload) in payloads {
            let (body, error) = match payload {
                Ok(body) => (body, None),
                Err(error) => (String::new(), Some(error)),
            };
            sqlx::query("INSERT INTO expense_deliveries(event_id,channel,payload,state,next_attempt_at,last_error,attempts) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(id).bind(channel).bind(body).bind(if error.is_some(){"failed"}else{"pending"}).bind(timestamp).bind(&error).bind(if error.is_some(){POLICY.attempts}else{0}).execute(&mut *tx).await?;
        }
    }
    sqlx::query("UPDATE expense_deliveries d SET state='cancelled' FROM expense_events e LEFT JOIN expense_subscriptions s ON s.id=e.subscription_id WHERE d.event_id=e.id AND d.state IN('pending','failed') AND (NOT $1 OR s.id IS NULL OR s.inactive OR NOT s.notify)").bind(reminders).execute(&mut *tx).await?;
    tx.commit().await?;
    if reminders {
        deliver(state, &settings).await?;
    }
    Ok(())
}

async fn deliver(state: &AppState, settings: &settings::Settings) -> ApiResult<()> {
    queue::deliver(&Reminders { state, settings }).await
}

/// The renewal reminders due for the channels.
pub(super) struct Reminders<'a> {
    state: &'a AppState,
    settings: &'a settings::Settings,
}

pub(super) struct Reminder {
    id: i64,
    channel: String,
    payload: String,
}

impl Queue for Reminders<'_> {
    const POLICY: Policy = POLICY;
    type Message = Reminder;
    type Error = ApiError;

    fn claim(&self, claim: Claim) -> Claims<'_, Reminder, ApiError> {
        Box::pin(async move {
            let mut tx = self.state.pool.begin().await?;
            lock(&mut tx).await?;
            let rows=sqlx::query("SELECT d.id,d.channel,d.payload,d.attempts FROM expense_deliveries d JOIN expense_events e ON e.id=d.event_id JOIN expense_subscriptions s ON s.id=e.subscription_id WHERE (SELECT reminders_enabled FROM expense_settings WHERE singleton) AND NOT s.inactive AND s.notify AND d.attempts<$2 AND ((d.state IN('pending','failed') AND d.next_attempt_at<=$1) OR (d.state='sending' AND d.lease_until<$1)) ORDER BY d.id LIMIT $3 FOR UPDATE OF d SKIP LOCKED").bind(claim.now).bind(Self::POLICY.attempts).bind(claim.limit).fetch_all(&mut *tx).await?;
            let mut claimed = Vec::new();
            for row in rows {
                let id: i64 = row.try_get("id")?;
                sqlx::query("UPDATE expense_deliveries SET state='sending',attempts=attempts+1,lease_until=$2 WHERE id=$1").bind(id).bind(claim.lease_until).execute(&mut *tx).await?;
                claimed.push(Claimed {
                    message: Reminder {
                        id,
                        channel: row.try_get("channel")?,
                        payload: row.try_get("payload")?,
                    },
                    attempts: row.try_get("attempts")?,
                });
            }
            tx.commit().await?;
            Ok(claimed)
        })
    }

    fn send<'a>(&'a self, reminder: &'a Reminder) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(delivery::send(
            self.settings,
            &reminder.channel,
            &reminder.payload,
        ))
    }

    fn settle(&self, reminder: Reminder, outcome: Outcome) -> BoxFuture<'_, ApiResult<()>> {
        Box::pin(async move {
            let (state, next) = if outcome.sent() {
                ("sent", outcome.finished_at)
            } else {
                ("failed", outcome.retry_at)
            };
            sqlx::query("UPDATE expense_deliveries SET state=$2,last_error=$3,next_attempt_at=$4,lease_until=NULL WHERE id=$1 AND state='sending'").bind(reminder.id).bind(state).bind(&outcome.error).bind(next).execute(&self.state.pool).await?;
            Ok(())
        })
    }
}

pub(super) async fn notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    actor(&state, &headers, false).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',e.id,'subscription_id',e.subscription_id,'kind',e.kind,'detail',e.detail,'created_at',e.created_at,'deliveries',COALESCE((SELECT jsonb_agg(jsonb_build_object('channel',d.channel,'state',d.state,'attempts',d.attempts,'last_error',d.last_error)) FROM expense_deliveries d WHERE d.event_id=e.id),'[]'::jsonb)) FROM expense_events e ORDER BY e.created_at DESC,e.id LIMIT 200").fetch_all(&state.pool).await?;
    Ok(Json(json!({"entries":rows})))
}
