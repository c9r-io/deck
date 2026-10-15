//! Static first-party extension composition: the ONE place that says which
//! service integrations, agent adapters and local capability providers this
//! build of Deck contains.
//!
//! # Contract
//! - Three typed tables, no common interface: `SERVICES` (service
//!   integrations; today `slack::INTEGRATION`), `AGENT_ADAPTERS` (the
//!   declarations in `agent_status/hooks.rs` (re-exported by `agent_status`) and `agent_pi.rs`) and `TRANSLATION_PROVIDERS` (the
//!   engines behind `intelligence::provider::TranslationProvider`), plus the
//!   one engine this build selects, `TRANSLATION_ENGINE`.
//! - Entries are source code compiled into the app. No setting, environment
//!   variable, path or download adds, removes or reorders one; the only
//!   override is the `cfg(test)` thread-local table the routing tests use.
//!   An entry has the application's own trust level: the tables are an
//!   architectural boundary, not a sandbox, and Deck has no plugin loader.
//! - Being registered is not being enabled, connected, loaded, trusted or
//!   authorized. A table lookup starts nothing: a service connects only
//!   under its own saved settings and credentials (`slack_transport.rs`),
//!   a provider is built on the first translation and loads its model only
//!   then (`intelligence/translation.rs`), and an agent adapter's source
//!   word earns Signal only through the pane, generation and interaction
//!   checks of `agent_status::ingest` — for Pi, the peer, lease and order
//!   checks of `agent_pi::transport`. Remote admission (`admission.rs`)
//!   does not read these tables.
//! - No silent fallback, and no winner in a clash: an identity is used only
//!   when EXACTLY one entry carries it, never the first or the last of
//!   several. That holds for every consumer, not only the lookups: an
//!   unknown or doubly registered id is `None` / an error at every caller;
//!   a service id two entries share is left out of start, wake,
//!   re-announcement, source contribution and credential ownership
//!   (`unambiguous_services`); a source name is polled only when one
//!   contributed source carries it, `inbound::SOURCES` names it and it is
//!   not one the dispatcher supplies itself (`inbound::BUILT_IN_SOURCES`,
//!   the clock); a whole-table agent pass skips a shared source word
//!   (`unambiguous_agents`). A clash removes only the identities in it.
//!   `check` states the whole rule for the unit test that pins the
//!   production tables, and a malformed table starts no connection owner.
//! - A service integration hands the core only what the fields below name.
//!   It gets no Board, queue, PTY or authority entry from this module; the
//!   dispatcher (`inbound.rs`), the settings writer and the scheduler keep
//!   every decision they had.
//!
//! Adding a future extension of an existing kind = its own module + one
//! source manifest compiled into the generated matching table (a service's
//! trigger source also needs its closed name in `inbound::SOURCES`, which is
//! a persisted settings vocabulary and therefore a core decision). See
//! `docs/first-party-extensions.md`.

mod generated;

use crate::agent_status::AgentAdapter;
use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::inbound::Source;
use crate::intelligence::provider::{TranslationProvider, TranslationProviderEntry};
use crate::keychain::Slot;
use generated::{AGENT_ADAPTERS, SERVICES, TRANSLATION_PROVIDERS};
use tauri::{AppHandle, Emitter};

/// The generated build selection, kept at the existing core module path.
pub(crate) const TRANSLATION_ENGINE: &str = generated::TRANSLATION_ENGINE;

/// One external service: its one connection owner, the trigger sources it
/// contributes to the dispatcher, and its credential adaptation.
pub(crate) struct ServiceIntegration {
    /// Stable id; equals the dispatcher source name its status is shown under.
    pub(crate) id: &'static str,
    /// The trigger sources this service contributes to the dispatcher.
    pub(crate) sources: fn() -> Vec<Box<dyn Source>>,
    /// Starts the service's ONE connection owner. The owner itself decides,
    /// from saved settings and credentials, whether it ever connects.
    pub(crate) start: fn(AppHandle),
    /// Makes the owner re-read settings and credentials now.
    pub(crate) wake: fn(),
    /// Whether the owner's live connection is up at this instant.
    pub(crate) live: fn() -> bool,
    /// Keychain slots whose PRESENCE the status view reports (wire name, slot).
    pub(crate) secrets: &'static [(&'static str, Slot)],
    /// Slots the user may store or clear through `inbound_set_secret`.
    pub(crate) settable: &'static [Slot],
    /// Verifies a candidate with the service, then writes the Keychain slot.
    pub(crate) store_secret: fn(Slot, &str) -> Result<(), DeckError>,
    /// The compiled-in "create an app" page of the service.
    pub(crate) setup_url: fn() -> String,
    /// Content-free re-announcement of the service's own durable pending
    /// work: the service names the event, the host emits it.
    pub(crate) reannounce: fn(&dyn Fn(&'static str)),
}

#[cfg(test)]
thread_local! {
    static TEST_SERVICES: std::cell::Cell<Option<&'static [ServiceIntegration]>> =
        const { std::cell::Cell::new(None) };
    static TEST_AGENTS: std::cell::Cell<Option<&'static [AgentAdapter]>> =
        const { std::cell::Cell::new(None) };
    static TEST_PROVIDERS: std::cell::Cell<Option<&'static [TranslationProviderEntry]>> =
        const { std::cell::Cell::new(None) };
}

pub(crate) fn services() -> &'static [ServiceIntegration] {
    #[cfg(test)]
    if let Some(table) = TEST_SERVICES.with(|cell| cell.get()) {
        return table;
    }
    SERVICES
}

fn agents() -> &'static [AgentAdapter] {
    #[cfg(test)]
    if let Some(table) = TEST_AGENTS.with(|cell| cell.get()) {
        return table;
    }
    AGENT_ADAPTERS
}

fn translation_providers() -> &'static [TranslationProviderEntry] {
    #[cfg(test)]
    if let Some(table) = TEST_PROVIDERS.with(|cell| cell.get()) {
        return table;
    }
    TRANSLATION_PROVIDERS
}

/// The entry carrying `id`, only when exactly one does.
fn exactly_one<'a, T>(table: &'a [T], id: &str, of: impl Fn(&T) -> &'static str) -> Option<&'a T> {
    let mut found = table.iter().filter(|entry| of(entry) == id);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

pub(crate) fn service(id: &str) -> Option<&'static ServiceIntegration> {
    exactly_one(services(), id, |s| s.id)
}

/// The services every dispatch reaches, in table order: an id two entries
/// share selects neither of them.
fn unambiguous_services() -> impl Iterator<Item = &'static ServiceIntegration> {
    services().iter().filter(|s| service(s.id).is_some())
}

/// The service that lets the user store `slot`, only when exactly one does.
pub(crate) fn secret_owner(slot: Slot) -> Option<&'static ServiceIntegration> {
    let mut owners = unambiguous_services().filter(|s| s.settable.contains(&slot));
    let first = owners.next()?;
    owners.next().is_none().then_some(first)
}

pub(crate) fn agent(source: &str) -> Option<&'static AgentAdapter> {
    exactly_one(agents(), source, |a| a.source)
}

/// The adapters a whole-table pass visits, in table order: a source word
/// two entries share selects neither of them.
pub(crate) fn unambiguous_agents() -> impl Iterator<Item = &'static AgentAdapter> {
    agents().iter().filter(|a| agent(a.source).is_some())
}

/// Builds the selected engine. Nothing is loaded: the coordinator loads the
/// model on the first translation that needs it.
pub(crate) fn translation_provider() -> Result<Box<dyn TranslationProvider>, DeckError> {
    exactly_one(translation_providers(), TRANSLATION_ENGINE, |p| p.id)
        .map(|entry| (entry.build)())
        .ok_or(DeckError::new(ErrorKind::Other, "translation-failed"))
}

/// Every trigger source the registered services contribute, in table order.
/// A name is polled only when exactly one contributed source carries it, the
/// dispatcher's closed vocabulary names it and the host does not supply it
/// itself; every other source is refused here, never the first or the last
/// of a clash.
pub(crate) fn service_sources() -> Vec<Box<dyn Source>> {
    if services().len() != unambiguous_services().count() {
        applog("[extensions] service refused (duplicate-id)");
    }
    let all: Vec<Box<dyn Source>> = unambiguous_services()
        .flat_map(|service| (service.sources)())
        .collect();
    let names: Vec<&'static str> = all.iter().map(|source| source.id()).collect();
    all.into_iter()
        .filter(|source| {
            let id = source.id();
            let refused = if !crate::inbound::SOURCES.contains(&id) {
                Some("unknown-source")
            } else if crate::inbound::BUILT_IN_SOURCES.contains(&id) {
                Some("reserved-source")
            } else if names.iter().filter(|name| **name == id).count() != 1 {
                Some("duplicate-source")
            } else {
                None
            };
            if let Some(code) = refused {
                applog(&format!("[extensions] source refused ({code})"));
            }
            refused.is_none()
        })
        .collect()
}

/// The production tables against the well-formedness rule.
pub(crate) fn check() -> Result<(), &'static str> {
    check_tables(
        services(),
        agents(),
        translation_providers(),
        TRANSLATION_ENGINE,
    )
}

/// Start every registered service's connection owner, once each. Malformed
/// tables start nothing.
pub(crate) fn start_services(app: &AppHandle) {
    if let Err(code) = check() {
        applog(&format!("[extensions] services not started ({code})"));
        return;
    }
    let mut started = 0usize;
    for service in unambiguous_services() {
        (service.start)(app.clone());
        started += 1;
    }
    // Content-free: a count, never a service name or a credential fact.
    applog(&format!("[extensions] services started ({started})"));
}

pub(crate) fn wake_services() {
    for service in unambiguous_services() {
        (service.wake)();
    }
}

/// One pass over the services with durable pending work of their own; each
/// names the content-free event the window is sent.
pub(crate) fn reannounce_services<R: tauri::Runtime>(app: &AppHandle<R>) {
    for service in unambiguous_services() {
        (service.reannounce)(&|event| {
            let _ = app.emit(event, ());
        });
    }
}

/// The whole well-formedness rule over three tables, as one closed code.
fn check_tables(
    services: &[ServiceIntegration],
    agents: &[AgentAdapter],
    providers: &[TranslationProviderEntry],
    engine: &str,
) -> Result<(), &'static str> {
    fn unique(ids: &[&str]) -> bool {
        ids.iter()
            .enumerate()
            .all(|(i, id)| !id.is_empty() && !ids[..i].contains(id))
    }
    let service_ids: Vec<&str> = services.iter().map(|s| s.id).collect();
    if !unique(&service_ids) {
        return Err("duplicate-service");
    }
    let mut source_ids: Vec<&'static str> = Vec::new();
    let mut slots: Vec<Slot> = Vec::new();
    for service in services {
        for source in (service.sources)() {
            source_ids.push(source.id());
        }
        for slot in service.settable {
            if slots.contains(slot) {
                return Err("duplicate-secret-owner");
            }
            slots.push(*slot);
        }
    }
    if !unique(&source_ids) {
        return Err("duplicate-source");
    }
    if source_ids
        .iter()
        .any(|id| crate::inbound::BUILT_IN_SOURCES.contains(id))
    {
        return Err("reserved-source");
    }
    if source_ids
        .iter()
        .any(|id| !crate::inbound::SOURCES.contains(id))
    {
        return Err("unknown-source");
    }
    if !unique(&agents.iter().map(|a| a.source).collect::<Vec<_>>()) {
        return Err("duplicate-agent");
    }
    if !unique(&providers.iter().map(|p| p.id).collect::<Vec<_>>()) {
        return Err("duplicate-provider");
    }
    if !providers.iter().any(|p| p.id == engine) {
        return Err("engine-unregistered");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod testing {
    //! Thread-local table overrides for routing tests. Test builds only: no
    //! production code path can install a table.
    use super::*;

    pub(crate) struct Restore(fn());
    impl Drop for Restore {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    pub(crate) fn with_services(table: &'static [ServiceIntegration]) -> Restore {
        TEST_SERVICES.with(|cell| cell.set(Some(table)));
        Restore(|| TEST_SERVICES.with(|cell| cell.set(None)))
    }
    pub(crate) fn with_agents(table: &'static [AgentAdapter]) -> Restore {
        TEST_AGENTS.with(|cell| cell.set(Some(table)));
        Restore(|| TEST_AGENTS.with(|cell| cell.set(None)))
    }
    pub(crate) fn with_providers(table: &'static [TranslationProviderEntry]) -> Restore {
        TEST_PROVIDERS.with(|cell| cell.set(Some(table)));
        Restore(|| TEST_PROVIDERS.with(|cell| cell.set(None)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbound::{Config, Event, SourceStatus};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Named(&'static str);
    impl Source for Named {
        fn id(&self) -> &'static str {
            self.0
        }
        fn enabled(&self, _: &Config) -> bool {
            false
        }
        fn poll(&mut self, _: &Config, _: &[String]) -> Result<Vec<Event>, &'static str> {
            Ok(Vec::new())
        }
        fn set_live(&mut self, _: &AppHandle, _: bool, _: &[String]) {}
        fn status(&self) -> SourceStatus {
            SourceStatus::default()
        }
    }

    static WAKES: AtomicUsize = AtomicUsize::new(0);
    static REANNOUNCES: AtomicUsize = AtomicUsize::new(0);
    const fn fake(id: &'static str, sources: fn() -> Vec<Box<dyn Source>>) -> ServiceIntegration {
        ServiceIntegration {
            id,
            sources,
            start: |_| {},
            wake: || {
                WAKES.fetch_add(1, Ordering::SeqCst);
            },
            live: || false,
            secrets: &[],
            settable: &[],
            store_secret: |_, _| Ok(()),
            setup_url: String::new,
            reannounce: |announce| {
                REANNOUNCES.fetch_add(1, Ordering::SeqCst);
                announce("fake-changed");
            },
        }
    }
    fn slack_source() -> Vec<Box<dyn Source>> {
        vec![Box::new(Named("slack"))]
    }
    fn unknown_source() -> Vec<Box<dyn Source>> {
        vec![Box::new(Named("notion"))]
    }
    fn repeated_source() -> Vec<Box<dyn Source>> {
        vec![Box::new(Named("slack")), Box::new(Named("slack"))]
    }

    #[test]
    fn production_tables_are_well_formed_and_closed() {
        assert_eq!(
            check_tables(
                SERVICES,
                AGENT_ADAPTERS,
                TRANSLATION_PROVIDERS,
                TRANSLATION_ENGINE
            ),
            Ok(())
        );
        // the closed inventory of this build
        assert_eq!(SERVICES.iter().map(|s| s.id).collect::<Vec<_>>(), ["slack"]);
        assert_eq!(
            AGENT_ADAPTERS.iter().map(|a| a.source).collect::<Vec<_>>(),
            ["claude-code", "codex", "pi"]
        );
        // registered is not re-entered: only Claude declares that capability
        assert_eq!(
            AGENT_ADAPTERS
                .iter()
                .filter(|a| a.reentry.is_some())
                .map(|a| a.source)
                .collect::<Vec<_>>(),
            ["claude-code"]
        );
        assert_eq!(
            TRANSLATION_PROVIDERS
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            ["bergamot"]
        );
        // services plus the built-in clock are exactly the dispatcher's names
        let mut names: Vec<&str> = service_sources().iter().map(|s| s.id()).collect();
        names.push("clock");
        assert_eq!(names, crate::inbound::SOURCES);
    }

    #[test]
    fn malformed_tables_are_named_not_tolerated() {
        const TWICE: &[ServiceIntegration] =
            &[fake("slack", slack_source), fake("slack", Vec::new)];
        const UNKNOWN: &[ServiceIntegration] = &[fake("notion", unknown_source)];
        const REPEATED: &[ServiceIntegration] = &[fake("slack", repeated_source)];
        let agents = AGENT_ADAPTERS;
        let providers = TRANSLATION_PROVIDERS;
        let run =
            |services, agents, providers, engine| check_tables(services, agents, providers, engine);
        assert_eq!(
            run(TWICE, agents, providers, "bergamot"),
            Err("duplicate-service")
        );
        assert_eq!(
            run(UNKNOWN, agents, providers, "bergamot"),
            Err("unknown-source")
        );
        assert_eq!(
            run(REPEATED, agents, providers, "bergamot"),
            Err("duplicate-source")
        );
        const TWO_OWNERS: &[ServiceIntegration] = &[
            ServiceIntegration {
                settable: &[Slot::SlackUserToken],
                ..fake("a", Vec::new)
            },
            ServiceIntegration {
                settable: &[Slot::SlackUserToken],
                ..fake("b", Vec::new)
            },
        ];
        assert_eq!(
            run(TWO_OWNERS, agents, providers, "bergamot"),
            Err("duplicate-secret-owner")
        );
        const AGENT_TWICE: &[AgentAdapter] =
            &[crate::agent_status::CODEX, crate::agent_status::CODEX];
        assert_eq!(
            run(SERVICES, AGENT_TWICE, providers, "bergamot"),
            Err("duplicate-agent")
        );
        const PROVIDER_TWICE: &[TranslationProviderEntry] = &[
            crate::intelligence::provider::BERGAMOT,
            crate::intelligence::provider::BERGAMOT,
        ];
        assert_eq!(
            run(SERVICES, agents, PROVIDER_TWICE, "bergamot"),
            Err("duplicate-provider")
        );
        assert_eq!(
            run(SERVICES, agents, providers, "other-engine"),
            Err("engine-unregistered")
        );
    }

    #[test]
    fn lookups_answer_only_for_exactly_one_entry() {
        assert_eq!(service("slack").map(|s| s.id), Some("slack"));
        assert!(service("notion").is_none());
        assert!(service("clock").is_none(), "the clock is not a service");
        assert_eq!(agent("codex").map(|a| a.source), Some("codex"));
        assert!(
            agent("claude").is_none(),
            "an executable name is not a source"
        );
        assert!(translation_provider().is_ok());
        {
            const TWICE: &[ServiceIntegration] =
                &[fake("slack", slack_source), fake("slack", slack_source)];
            let _table = testing::with_services(TWICE);
            assert!(service("slack").is_none(), "no first-wins lookup");
            assert!(service_sources().is_empty(), "neither duplicate is polled");
        }
        {
            const AGENT_TWICE: &[AgentAdapter] =
                &[crate::agent_status::CODEX, crate::agent_status::CODEX];
            let _table = testing::with_agents(AGENT_TWICE);
            assert!(agent("codex").is_none());
        }
        {
            const PROVIDER_TWICE: &[TranslationProviderEntry] = &[
                crate::intelligence::provider::BERGAMOT,
                crate::intelligence::provider::BERGAMOT,
            ];
            let _table = testing::with_providers(PROVIDER_TWICE);
            assert!(translation_provider().is_err());
        }
        {
            let _table = testing::with_providers(&[]);
            assert!(
                translation_provider().is_err(),
                "an unregistered engine is an error, never a built-in default"
            );
        }
        assert!(
            service("slack").is_some(),
            "the override ends with its guard"
        );
    }

    fn clock_source() -> Vec<Box<dyn Source>> {
        vec![Box::new(Named("clock"))]
    }
    fn polled() -> Vec<&'static str> {
        service_sources().iter().map(|s| s.id()).collect()
    }

    #[test]
    fn a_source_name_is_polled_only_when_exactly_one_registered_source_carries_it() {
        {
            const UNKNOWN: &[ServiceIntegration] =
                &[fake("notion", unknown_source), fake("slack", slack_source)];
            let _table = testing::with_services(UNKNOWN);
            assert_eq!(polled(), ["slack"], "the unknown name alone is refused");
        }
        {
            const REPEATED: &[ServiceIntegration] = &[fake("slack", repeated_source)];
            let _table = testing::with_services(REPEATED);
            assert!(polled().is_empty(), "one service, the same name twice");
        }
        {
            const SHARED: &[ServiceIntegration] =
                &[fake("a", slack_source), fake("b", slack_source)];
            let _table = testing::with_services(SHARED);
            assert!(polled().is_empty(), "two services, one name");
        }
        {
            // the host appends its own clock after the service sources
            const CLOCK: &[ServiceIntegration] =
                &[fake("a", clock_source), fake("slack", slack_source)];
            let _table = testing::with_services(CLOCK);
            assert_eq!(polled(), ["slack"], "a service never supplies the clock");
            assert_eq!(
                check_tables(CLOCK, AGENT_ADAPTERS, TRANSLATION_PROVIDERS, "bergamot"),
                Err("reserved-source")
            );
        }
    }

    #[test]
    fn a_service_id_two_entries_share_takes_no_part_in_dispatch() {
        const TWO: &[ServiceIntegration] = &[fake("a", Vec::new), fake("b", Vec::new)];
        const SHARED: &[ServiceIntegration] = &[
            fake("a", Vec::new),
            ServiceIntegration {
                settable: &[Slot::SlackUserToken],
                ..fake("slack", slack_source)
            },
            fake("slack", Vec::new),
        ];
        let woken = |table: &'static [ServiceIntegration]| {
            let _table = testing::with_services(table);
            let before = WAKES.load(Ordering::SeqCst);
            wake_services();
            WAKES.load(Ordering::SeqCst) - before
        };
        assert_eq!(woken(TWO), 2, "every distinct service is woken once");
        assert_eq!(woken(SHARED), 1, "neither entry of the shared id is woken");
        // the host emits what each remaining service names, once per pass
        let app = tauri::test::mock_app();
        let heard = std::sync::Arc::new(AtomicUsize::new(0));
        let count = heard.clone();
        tauri::Listener::listen(app.handle(), "fake-changed", move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let announced = |table: &'static [ServiceIntegration]| {
            let _table = testing::with_services(table);
            let (calls, events) = (
                REANNOUNCES.load(Ordering::SeqCst),
                heard.load(Ordering::SeqCst),
            );
            reannounce_services(app.handle());
            (
                REANNOUNCES.load(Ordering::SeqCst) - calls,
                heard.load(Ordering::SeqCst) - events,
            )
        };
        assert_eq!(announced(TWO), (2, 2));
        assert_eq!(announced(SHARED), (1, 1));
        assert_eq!(announced(&[]), (0, 0));
        {
            const AGENT_TWICE: &[AgentAdapter] = &[
                crate::agent_status::CODEX,
                crate::agent_status::CLAUDE_CODE,
                crate::agent_status::CODEX,
            ];
            let _table = testing::with_agents(AGENT_TWICE);
            let visited: Vec<&str> = unambiguous_agents().map(|a| a.source).collect();
            assert_eq!(visited, ["claude-code"], "a whole-table pass skips both");
        }
        let _table = testing::with_services(SHARED);
        assert!(polled().is_empty());
        assert!(
            secret_owner(Slot::SlackUserToken).is_none(),
            "an ambiguous service owns no credential slot"
        );
    }
}
