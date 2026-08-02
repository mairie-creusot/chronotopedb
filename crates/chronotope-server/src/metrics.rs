//! Compteurs Prometheus minimalistes, formates a la main : le besoin (une
//! poignee de compteurs et une latence moyenne par route) ne justifie pas une
//! dependance dediee (`metrics` + `metrics-exporter-prometheus`) qui
//! doublerait la pile HTTP pour six chiffres. Le format d'exposition
//! Prometheus est un texte trivial (`# HELP`/`# TYPE`/`nom{labels} valeur`) —
//! zero nouvelle dependance.
//!
//! Protegee par le meme secret que `/write`/`/seal`/`/read` (voir
//! `main.rs::authentifier`) : contrairement au `/metrics` de SpacetimeDB
//! (dont le middleware d'auth est reste en TODO jamais branche), il n'y a ici
//! aucune raison de laisser une fenetre non protegee, meme temporaire.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Default)]
pub struct RouteCounters {
    ok: AtomicU64,
    err: AtomicU64,
    latence_us_total: AtomicU64,
}

impl RouteCounters {
    /// Enregistre une requete terminee. `succes` reflete le RESULTAT
    /// SEMANTIQUE (ex. `WriteError::AlreadySealed` compte comme un echec
    /// meme si la reponse HTTP reste `200` avec `{ok:false}` dans le corps —
    /// le statut HTTP seul ne suffit pas a distinguer les deux ici).
    pub fn observer(&self, succes: bool, duree: Duration) {
        if succes {
            self.ok.fetch_add(1, Ordering::Relaxed);
        } else {
            self.err.fetch_add(1, Ordering::Relaxed);
        }
        self.latence_us_total
            .fetch_add(duree.as_micros() as u64, Ordering::Relaxed);
    }

    fn total(&self) -> u64 {
        self.ok.load(Ordering::Relaxed) + self.err.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct Metrics {
    pub write: RouteCounters,
    pub seal: RouteCounters,
    pub read: RouteCounters,
    pub auth_rejected: AtomicU64,
}

impl Metrics {
    pub fn render_prometheus(&self) -> String {
        let routes: [(&str, &RouteCounters); 3] = [
            ("write", &self.write),
            ("seal", &self.seal),
            ("read", &self.read),
        ];
        let mut sortie = String::new();

        writeln!(
            sortie,
            "# HELP chronotope_requests_total Nombre de requetes par route et resultat."
        )
        .unwrap();
        writeln!(sortie, "# TYPE chronotope_requests_total counter").unwrap();
        for (nom, compteurs) in routes {
            writeln!(
                sortie,
                "chronotope_requests_total{{route=\"{nom}\",result=\"ok\"}} {}",
                compteurs.ok.load(Ordering::Relaxed)
            )
            .unwrap();
            writeln!(
                sortie,
                "chronotope_requests_total{{route=\"{nom}\",result=\"err\"}} {}",
                compteurs.err.load(Ordering::Relaxed)
            )
            .unwrap();
        }

        writeln!(
            sortie,
            "# HELP chronotope_request_duration_seconds_sum Somme des latences observees."
        )
        .unwrap();
        writeln!(
            sortie,
            "# TYPE chronotope_request_duration_seconds_sum counter"
        )
        .unwrap();
        for (nom, compteurs) in routes {
            let secondes = compteurs.latence_us_total.load(Ordering::Relaxed) as f64 / 1_000_000.0;
            writeln!(
                sortie,
                "chronotope_request_duration_seconds_sum{{route=\"{nom}\"}} {secondes:.6}"
            )
            .unwrap();
        }

        writeln!(
            sortie,
            "# HELP chronotope_request_duration_seconds_count Nombre d'observations."
        )
        .unwrap();
        writeln!(
            sortie,
            "# TYPE chronotope_request_duration_seconds_count counter"
        )
        .unwrap();
        for (nom, compteurs) in routes {
            writeln!(
                sortie,
                "chronotope_request_duration_seconds_count{{route=\"{nom}\"}} {}",
                compteurs.total()
            )
            .unwrap();
        }

        writeln!(
            sortie,
            "# HELP chronotope_auth_rejected_total Requetes rejetees par le secret partage (401)."
        )
        .unwrap();
        writeln!(sortie, "# TYPE chronotope_auth_rejected_total counter").unwrap();
        writeln!(
            sortie,
            "chronotope_auth_rejected_total {}",
            self.auth_rejected.load(Ordering::Relaxed)
        )
        .unwrap();

        sortie
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_prometheus_reflete_les_incrementations() {
        let m = Metrics::default();
        m.write.observer(true, Duration::from_millis(10));
        m.write.observer(true, Duration::from_millis(20));
        m.write.observer(false, Duration::from_millis(5));
        m.auth_rejected.fetch_add(2, Ordering::Relaxed);

        let texte = m.render_prometheus();
        assert!(texte.contains("chronotope_requests_total{route=\"write\",result=\"ok\"} 2"));
        assert!(texte.contains("chronotope_requests_total{route=\"write\",result=\"err\"} 1"));
        assert!(texte.contains("chronotope_requests_total{route=\"seal\",result=\"ok\"} 0"));
        assert!(texte.contains("chronotope_request_duration_seconds_count{route=\"write\"} 3"));
        assert!(texte.contains("chronotope_auth_rejected_total 2"));
    }

    #[test]
    fn compteurs_vides_par_defaut() {
        let m = Metrics::default();
        let texte = m.render_prometheus();
        assert!(texte.contains("chronotope_requests_total{route=\"write\",result=\"ok\"} 0"));
        assert!(texte.contains("chronotope_request_duration_seconds_sum{route=\"read\"} 0.000000"));
        assert!(texte.contains("chronotope_auth_rejected_total 0"));
    }

    #[test]
    fn latence_totale_cumule_plusieurs_observations() {
        let c = RouteCounters::default();
        c.observer(true, Duration::from_micros(1_500_000));
        c.observer(true, Duration::from_micros(500_000));
        assert_eq!(c.latence_us_total.load(Ordering::Relaxed), 2_000_000);
        assert_eq!(c.total(), 2);
    }
}
