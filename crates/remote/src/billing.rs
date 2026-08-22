#[derive(Clone)]
pub struct BillingService;

impl BillingService {
    pub fn new() -> Self {
        Self
    }

    pub fn is_configured(&self) -> bool {
        false
    }
}

impl Default for BillingService {
    fn default() -> Self {
        Self::new()
    }
}
