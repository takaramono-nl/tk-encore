use serde::Serialize;

use crate::store::Cohort;

#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub campaign_id: i64,
    pub campaign_name: String,
    pub campaign_status: String,
    pub source_lists: Vec<String>,
    pub strategy: String,
    pub cutoff: String,
    pub recipients: usize,
    pub excluded_local: usize,
    pub min_new: i64,
    pub dry_run: bool,
}

pub fn render_run(r: &RunReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "campaign {}  \"{}\"  ({})\n",
        r.campaign_id, r.campaign_name, r.campaign_status
    ));
    out.push_str(&format!(
        "source lists  {}\n",
        if r.source_lists.is_empty() {
            "—".to_string()
        } else {
            r.source_lists.join(", ")
        }
    ));
    out.push_str(&format!("strategy      {}\n", r.strategy));
    out.push_str(&format!("cutoff        {}\n", r.cutoff));
    out.push_str("────────────────────────────────────────────────\n");
    out.push_str(&format!("recipients    {}\n", num(r.recipients as i64)));
    out.push_str(&format!(
        "excluded      {} already served / in flight\n",
        num(r.excluded_local as i64)
    ));
    out.push_str(&format!("minimum       {} new\n", num(r.min_new)));
    if r.dry_run {
        let action = if r.recipients as i64 >= r.min_new && r.recipients > 0 {
            "send"
        } else {
            "hold (below minimum)"
        };
        out.push_str(&format!("action        {action}\n"));
        out.push_str("mode          dry run (no cohort created)\n");
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct BackfillReport {
    pub campaign_id: i64,
    pub campaign_name: String,
    pub campaign_uuid: String,
    pub requested: usize,
    pub resolved: usize,
    pub already_served: usize,
    pub pending: usize,
    pub recorded: usize,
    pub unknown: Vec<String>,
    pub dry_run: bool,
}

pub fn render_backfill(r: &BackfillReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "campaign {}  \"{}\"\n",
        r.campaign_id, r.campaign_name
    ));
    out.push_str(&format!("ledger key    {}\n", r.campaign_uuid));
    out.push_str("strategy      backfill\n");
    out.push_str("────────────────────────────────────────────────\n");
    out.push_str(&format!("requested      {}\n", num(r.requested as i64)));
    out.push_str(&format!("resolved       {}\n", num(r.resolved as i64)));
    out.push_str(&format!(
        "already served {}\n",
        num(r.already_served as i64)
    ));
    if r.dry_run {
        out.push_str(&format!("would record   {}\n", num(r.pending as i64)));
        out.push_str("mode           dry run (nothing written)\n");
    } else {
        out.push_str(&format!("recorded       {}\n", num(r.recorded as i64)));
    }
    if !r.unknown.is_empty() {
        out.push_str(&format!(
            "unknown        {} (skipped): {}\n",
            r.unknown.len(),
            r.unknown.join(", ")
        ));
    }
    out
}

pub fn render_cohorts(cohorts: &[Cohort]) -> String {
    if cohorts.is_empty() {
        return "no cohorts yet\n".to_string();
    }

    let mut out = format!(
        "{:<5} {:<9} {:<16} {:>9} {:>8} {:>7}  {}\n",
        "ID", "CAMPAIGN", "STATE", "EXPECTED", "PENDING", "RESEND", "CREATED"
    );
    for c in cohorts {
        out.push_str(&format!(
            "{:<5} {:<9} {:<16} {:>9} {:>8} {:>7}  {}\n",
            c.id,
            c.campaign_id,
            c.state,
            num(c.expected_count),
            c.pending_list_id.map(num).unwrap_or_else(|| "—".into()),
            c.resend_campaign_id.map(num).unwrap_or_else(|| "—".into()),
            c.created_at
        ));
    }
    out
}

pub fn render_cohort_detail(c: &Cohort, member_count: i64) -> String {
    let lists = c
        .source_list_ids()
        .map(|ids| {
            ids.iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|_| c.source_list_ids.clone());

    format!(
        "cohort {id}\n\
         campaign      {campaign}\n\
         state         {state}\n\
         source lists  {lists}\n\
         cutoff        {cutoff}\n\
         expected      {expected}\n\
         members       {members}\n\
         pending list  {pending}\n\
         resend        {resend}\n\
         created       {created}\n\
         sent          {sent}\n\
         closed        {closed}\n\
         last error    {error}\n",
        id = c.id,
        campaign = c.campaign_id,
        state = c.state,
        lists = lists,
        cutoff = c.since_at,
        expected = num(c.expected_count),
        members = num(member_count),
        pending = c.pending_list_id.map(num).unwrap_or_else(|| "—".into()),
        resend = c.resend_campaign_id.map(num).unwrap_or_else(|| "—".into()),
        created = c.created_at,
        sent = c.sent_at.clone().unwrap_or_else(|| "—".into()),
        closed = c.closed_at.clone().unwrap_or_else(|| "—".into()),
        error = c.last_error.clone().unwrap_or_else(|| "—".into()),
    )
}

fn num(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(num(0), "0");
        assert_eq!(num(999), "999");
        assert_eq!(num(1_204), "1,204");
        assert_eq!(num(12_345_678), "12,345,678");
    }
}
