#![cfg_attr(not(ralgrum_private_backend), allow(dead_code))]

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidationError {
    pub(crate) field_name: String,
    pub(crate) message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let field_name = self.field_name.rsplit('.').next().unwrap_or_default();
        write!(formatter, "{field_name} - {}", self.message)
    }
}

pub(crate) struct AccessToken(String);

impl AccessToken {
    pub(crate) fn from_inner(token: String) -> Self {
        Self(token)
    }

    #[cfg(test)]
    pub(crate) fn new(token: String) -> Self {
        Self::from_inner(token)
    }

    pub(crate) fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccessToken")
            .field("token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountProfile {
    pub(crate) username: String,
    pub(crate) email: String,
    pub(crate) permanent_premium: bool,
    pub(crate) permanent_premium_status: &'static str,
    pub(crate) pass_expiration_display: String,
    pub(crate) pass_expiration_millis: Option<i64>,
    pub(crate) pass_status: PassStatus,
    pub(crate) pass_limit_exceeded: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PassStatus {
    Active,
    Inactive,
    Unknown,
}

impl PassStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Active => "Pass Active",
            Self::Inactive | Self::Unknown => "No Active Pass",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AccountError {
    IdentityUnavailable,
    TokenRequired,
    UserAlreadyExists,
    InvalidCredentials,
    EmailNotConfirmed,
    AccountBanned,
    Unauthorized,
    Forbidden,
    DeviceLimitExceeded,
    InvalidRefreshToken,
    UserNotFound,
    IncorrectPassword,
    PasswordAlreadyUsed,
    CodeNotFound,
    CodeCantBeApplied,
    CodeExpired,
    ValidationFailed(Option<Vec<ValidationError>>),
    PassLimitExceeded,
    Unknown,
    RateLimited,
    ServiceUnavailable,
    RequestTimedOut,
    NetworkUnavailable,
    InvalidResponse,
    MissingAccessToken,
    InvalidPaymentSelection,
}

impl AccountError {
    #[cfg(test)]
    pub(crate) fn describe_validation_errors(&self) -> Option<String> {
        let Self::ValidationFailed(Some(errors)) = self else {
            return None;
        };
        Some(
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

impl fmt::Display for AccountError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IdentityUnavailable => "The saved account identity could not be prepared.",
            Self::TokenRequired => "A Murglar login token is required.",
            Self::UserAlreadyExists => "A Murglar account with this email already exists.",
            Self::InvalidCredentials => "Invalid Murglar username or password.",
            Self::EmailNotConfirmed => "Confirm the Murglar account email before signing in.",
            Self::AccountBanned => "The Murglar account is locked.",
            Self::Unauthorized => "The Murglar session is no longer authorized.",
            Self::Forbidden => "Murglar refused access to this account.",
            Self::DeviceLimitExceeded => "Device limit reached. Contact the Murglar developers to resolve the registered devices, then retry this same login.",
            Self::InvalidRefreshToken => "The Murglar refresh token is invalid.",
            Self::UserNotFound => "The requested Murglar account was not found.",
            Self::IncorrectPassword => "The current Murglar password is incorrect.",
            Self::PasswordAlreadyUsed => "That password was already used for this Murglar account.",
            Self::CodeNotFound => "The requested Murglar code was not found.",
            Self::CodeCantBeApplied => "The requested Murglar code cannot be applied.",
            Self::CodeExpired => "The requested Murglar code has expired.",
            Self::ValidationFailed(_) => "Murglar rejected one or more account fields.",
            Self::PassLimitExceeded => "Murglar reports that this account has reached its current Pass usage limit. High-quality or blocked-track requests may be rejected.",
            Self::Unknown => "Murglar returned an unrecognized error.",
            Self::RateLimited => "Murglar is temporarily refusing requests. Try again later.",
            Self::ServiceUnavailable => "The Murglar service is temporarily unavailable.",
            Self::RequestTimedOut => "The Murglar request timed out.",
            Self::NetworkUnavailable => "Murglar could not be reached.",
            Self::InvalidResponse => "Murglar returned an invalid response.",
            Self::MissingAccessToken => "Murglar did not return a login token.",
            Self::InvalidPaymentSelection => "The Murglar payment selection is invalid.",
        })
    }
}

impl std::error::Error for AccountError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReferralStats {
    pub(crate) referral_code: String,
    pub(crate) reward_days: u64,
    pub(crate) invitee_count: usize,
    pub(crate) reward_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaymentAmount {
    pub(crate) sum: f64,
    pub(crate) currency: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaymentPlan {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) price: String,
    pub(crate) price_usd: Option<PaymentAmount>,
    pub(crate) price_rub: Option<PaymentAmount>,
    pub(crate) merchants: Vec<PaymentMerchant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PaymentMerchant {
    pub(crate) id: String,
    pub(crate) title: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaymentPlans {
    pub(crate) premium: Option<PaymentPlan>,
    pub(crate) subscriptions: Vec<PaymentPlan>,
    pub(crate) premium_short_description: String,
    pub(crate) cheapest_subscription_per_month_rub: Option<PaymentAmount>,
    pub(crate) cheapest_subscription_per_month_usd: Option<PaymentAmount>,
}

impl PaymentPlans {
    pub(crate) fn premium_description(&self) -> &str {
        let description = self.premium_short_description.trim();
        if description.is_empty() {
            "Permanently unlocks Murglar premium app features."
        } else {
            description
        }
    }

    pub(crate) fn best_subscription_id(&self) -> Option<&str> {
        let target = self
            .cheapest_subscription_per_month_rub
            .as_ref()
            .or(self.cheapest_subscription_per_month_usd.as_ref())?;
        if target.currency.is_empty() {
            return None;
        }
        let use_rub = target.currency == "RUB";
        let mut best: Option<(&str, f64)> = None;

        for plan in &self.subscriptions {
            let Some(months) = subscription_months(&plan.id) else {
                continue;
            };
            let price = if use_rub {
                plan.price_rub.as_ref()
            } else {
                plan.price_usd.as_ref()
            };
            let Some(price) = price else {
                continue;
            };
            let difference = (price.sum / f64::from(months) - target.sum).abs();
            if !difference.is_finite() {
                continue;
            }
            if best.is_none_or(|(_, best_difference)| difference < best_difference) {
                best = Some((plan.id.as_str(), difference));
            }
        }

        best.map(|(id, _)| id)
    }
}

#[derive(Debug)]
pub(crate) struct AccountExtras {
    pub(crate) referral: Result<ReferralStats, AccountError>,
    pub(crate) plans: Result<PaymentPlans, AccountError>,
}

fn subscription_months(id: &str) -> Option<u32> {
    let id = id.to_ascii_uppercase();
    let stem = id.strip_suffix("_MONTH")?;
    let marker = ["SUBSCRIPTION_", "SUB_"]
        .iter()
        .filter_map(|marker| stem.rfind(marker).map(|index| (index, marker.len())))
        .max_by_key(|(index, _)| *index)?;
    let digits = &stem[marker.0 + marker.1..];
    (!digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::subscription_months;

    #[test]
    fn subscription_month_suffix_parsing_matches_reference_shapes() {
        assert_eq!(subscription_months("SUB_3_MONTH"), Some(3));
        assert_eq!(subscription_months("subscription_12_month"), Some(12));
        assert_eq!(subscription_months("prefix-SUB_6_MONTH"), Some(6));
        assert_eq!(subscription_months("SUB_3_MONTH_SUFFIX"), None);
        assert_eq!(subscription_months("SUB_3_MONTHS"), None);
    }
}
