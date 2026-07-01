use crate::{config::SmtpConfig, error::AppError};
use aws_sdk_sesv2::error::ProvideErrorMetadata;
use aws_sdk_sesv2::types::{Body, Content, Destination, EmailContent, Message as SesMessage};
use aws_sdk_sesv2::Client as SesClient;
use lettre::{
    message::header::ContentType, transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use tracing::Instrument;

pub type Mailer = AsyncSmtpTransport<Tokio1Executor>;

pub fn build_mailer(cfg: &SmtpConfig) -> Result<Mailer, AppError> {
    let creds = Credentials::new(cfg.username.clone(), cfg.password.clone());
    // Port 587 uses STARTTLS (plain → upgrade); port 465 uses implicit TLS.
    let builder = if cfg.port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)
            .map_err(|e| AppError::Email(format!("SMTP relay setup failed: {e}")))?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
            .map_err(|e| AppError::Email(format!("SMTP relay setup failed: {e}")))?
    };
    let mailer = builder.port(cfg.port).credentials(creds).build();
    Ok(mailer)
}

/// Send a plain-text email.
///
/// Creates an OTLP span tagged with messaging semantic conventions.
/// Callers should record `email_sends_total` on the `Metrics` struct with the
/// `status=ok|error` label based on this function's return value.
pub async fn send(
    mailer: &Mailer,
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<(), AppError> {
    let span = tracing::info_span!(
        "smtp.send",
        "messaging.system"   = "smtp",
        "messaging.operation"= "send",
        "email.to"           = to,
        "email.subject"      = subject,
        "otel.kind"          = "producer",
    );

    async {
        let email = Message::builder()
            .from(
                from.parse()
                    .map_err(|e| AppError::Email(format!("invalid from address: {e}")))?,
            )
            .to(to
                .parse()
                .map_err(|e| AppError::Email(format!("invalid to address: {e}")))?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_owned())
            .map_err(|e| AppError::Email(format!("failed to build email: {e}")))?;

        crate::logging::info_with(
            &[("to", to), ("provider", "smtp")],
            "sending email",
        );

        mailer
            .send(email)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "SMTP send failed");
                AppError::Email(format!("SMTP send failed: {e}"))
            })?;

        crate::logging::info_with(
            &[("to", to), ("provider", "smtp")],
            "email sent",
        );

        Ok(())
    }
    .instrument(span)
    .await
}

/// Compose and send a meeting invitation email.
pub async fn send_meeting_invitation(
    mailer: &Mailer,
    from: &str,
    to_email: &str,
    to_name: &str,
    meeting_title: &str,
    start: &chrono::DateTime<chrono::Utc>,
    end: &chrono::DateTime<chrono::Utc>,
    meeting_link: Option<&str>,
) -> Result<(), AppError> {
    let subject = format!("Meeting invitation: {}", meeting_title);
    let body = format!(
        "Hi {to_name},\n\nYou have been invited to: {title}\n\nTime: {start} – {end} UTC\n{link}\n\nSee you there!\n— PlanPal",
        to_name = to_name,
        title = meeting_title,
        start = start.format("%A, %d %B %Y %H:%M"),
        end = end.format("%H:%M"),
        link = meeting_link.map(|l| format!("\nJoin: {}", l)).unwrap_or_default(),
    );
    send(mailer, from, to_email, &subject, &body).await
}

/// Build an AWS SES v2 client.
///
/// Credentials are resolved via the default AWS credential chain:
/// environment variables (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`),
/// ECS task role, EC2 instance profile, etc.  The region is taken from
/// `AWS_REGION` / `AWS_DEFAULT_REGION`.
pub async fn build_ses_client() -> SesClient {
    let config = aws_config::load_from_env().await;
    SesClient::new(&config)
}

/// Send a meeting invitation email via AWS SES v2.
pub async fn send_meeting_invitation_ses(
    client: &SesClient,
    from: &str,
    to_email: &str,
    to_name: &str,
    meeting_title: &str,
    start: &chrono::DateTime<chrono::Utc>,
    end: &chrono::DateTime<chrono::Utc>,
    meeting_link: Option<&str>,
) -> Result<(), AppError> {
    let span = tracing::info_span!(
        "ses.send",
        "messaging.system"    = "ses",
        "messaging.operation" = "send",
        "email.to"            = to_email,
        "email.subject"       = %format!("Meeting invitation: {}", meeting_title),
        "otel.kind"           = "producer",
    );

    async {
        let subject = format!("Meeting invitation: {}", meeting_title);
        let body = format!(
            "Hi {to_name},\n\nYou have been invited to: {title}\n\nTime: {start} – {end} UTC\n{link}\n\nSee you there!\n— PlanPal",
            to_name = to_name,
            title = meeting_title,
            start = start.format("%A, %d %B %Y %H:%M"),
            end = end.format("%H:%M"),
            link = meeting_link.map(|l| format!("\nJoin: {}", l)).unwrap_or_default(),
        );

        let body_content = Content::builder()
            .data(body)
            .charset("UTF-8")
            .build()
            .map_err(|e| AppError::Email(format!("SES body build failed: {e}")))?;

        let subject_content = Content::builder()
            .data(subject)
            .charset("UTF-8")
            .build()
            .map_err(|e| AppError::Email(format!("SES subject build failed: {e}")))?;

        let message = SesMessage::builder()
            .subject(subject_content)
            .body(Body::builder().text(body_content).build())
            .build();

        let dest = Destination::builder()
            .to_addresses(to_email)
            .build();

        let email_content = EmailContent::builder()
            .simple(message)
            .build();

        crate::logging::info_with(
            &[("to", to_email), ("provider", "ses")],
            "sending email",
        );

        client
            .send_email()
            .from_email_address(from)
            .destination(dest)
            .content(email_content)
            .send()
            .await
            .map_err(|e| {
                let meta = e.meta();
                AppError::Email(format!(
                    "SES send failed: code={} message={}",
                    meta.code().unwrap_or("unknown"),
                    meta.message().unwrap_or("no details"),
                ))
            })?;

        crate::logging::info_with(
            &[("to", to_email), ("provider", "ses")],
            "email sent",
        );

        Ok(())
    }
    .instrument(span)
    .await
}
