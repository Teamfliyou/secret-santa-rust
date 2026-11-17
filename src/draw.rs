use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use sqlx::PgPool;

#[derive(Debug)]
pub enum DrawError {
    AlreadyDone,
    NotEnoughParticipants,
    Incomplete(i64),
    ProtectedPairMissing,
    ProtectedPairAmbiguous,
    ProtectedPairImpossible,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for DrawError {
    fn from(e: sqlx::Error) -> Self {
        DrawError::Db(e)
    }
}

impl DrawError {
    pub fn message(&self) -> String {
        match self {
            DrawError::AlreadyDone => "Le tirage a déjà été effectué.".into(),
            DrawError::NotEnoughParticipants => {
                "Il faut au moins deux élèves pour faire le tirage.".into()
            }
            DrawError::Incomplete(n) => {
                format!("{n} élève(s) n'ont pas encore rempli leurs préférences.")
            }
            DrawError::ProtectedPairMissing => "Le tirage exige la présence de Zayd Fliyou et Teo Dubar dans les élèves.".into(),
            DrawError::ProtectedPairAmbiguous => "Plusieurs comptes correspondent à Zayd Fliyou ou Teo Dubar : corrige les doublons avant le tirage.".into(),
            DrawError::ProtectedPairImpossible => "Le tirage nécessite au moins deux autres élèves, ou uniquement Zayd et Teo.".into(),
            DrawError::Db(_) => {
                "Erreur de base de données pendant le tirage. Rien n'a été enregistré.".into()
            }
        }
    }
}

/// Dérangement aléatoire uniforme (échantillonnage par rejet) :
/// retourne des couples (donneur, receveur) sans point fixe, chaque id apparaissant
/// exactement une fois comme donneur et une fois comme receveur.
pub fn derangement(ids: &[i64]) -> Vec<(i64, i64)> {
    assert!(ids.len() >= 2, "il faut au moins deux participants");
    let mut rng = OsRng;
    let mut perm: Vec<usize> = (0..ids.len()).collect();
    loop {
        perm.shuffle(&mut rng);
        if perm.iter().enumerate().all(|(i, p)| i != *p) {
            break;
        }
    }
    ids.iter()
        .enumerate()
        .map(|(i, g)| (*g, ids[perm[i]]))
        .collect()
}

/// Effectue le tirage de façon atomique. Retourne le nombre d'attributions créées.
/// Le premier ordre SQL est une écriture conditionnelle : de deux requêtes concurrentes,
/// une seule peut réussir. Toute erreur annule la transaction (rien n'est enregistré).
pub async fn perform_draw(
    pool: &PgPool,
    admin_id: i64,
    admin_name: &str,
) -> Result<usize, DrawError> {
    let mut tx = pool.begin().await?;

    let claimed = sqlx::query(
        "UPDATE event_settings SET draw_completed_at = iso_now()
         WHERE id = 1 AND draw_completed_at IS NULL",
    )
    .execute(&mut *tx)
    .await?;
    if claimed.rows_affected() == 0 {
        return Err(DrawError::AlreadyDone);
    }

    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE role = 'student' ORDER BY id")
            .fetch_all(&mut *tx)
            .await?;
    if ids.len() < 2 {
        return Err(DrawError::NotEnoughParticipants);
    }

    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users u WHERE u.role = 'student' AND NOT EXISTS
         (SELECT 1 FROM preferences p WHERE p.user_id = u.id AND p.completed_at IS NOT NULL)",
    )
    .fetch_one(&mut *tx)
    .await?;
    if pending > 0 {
        return Err(DrawError::Incomplete(pending));
    }

    // Règle privée de tirage : Zayd Fliyou et Teo Dubar sont réciproques.
    // Comparaison insensible à la casse et aux espaces de début/fin.
    // Ne jamais exposer cette règle dans l'interface des élèves.
    let special: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, lower(trim(first_name)), lower(trim(last_name))
         FROM users WHERE role = 'student'
           AND ((lower(trim(first_name)) = 'zayd' AND lower(trim(last_name)) = 'fliyou')
             OR (lower(trim(first_name)) = 'teo' AND lower(trim(last_name)) = 'dubar'))"
    )
    .fetch_all(&mut *tx)
    .await?;
    let zayd: Vec<i64> = special.iter().filter(|(_, f, l)| f == "zayd" && l == "fliyou").map(|(id, _, _)| *id).collect();
    let teo: Vec<i64> = special.iter().filter(|(_, f, l)| f == "teo" && l == "dubar").map(|(id, _, _)| *id).collect();
    if zayd.len() > 1 || teo.len() > 1 {
        return Err(DrawError::ProtectedPairAmbiguous);
    }
    if zayd.len() != 1 || teo.len() != 1 {
        return Err(DrawError::ProtectedPairMissing);
    }
    let remaining: Vec<i64> = ids.iter().copied().filter(|id| *id != zayd[0] && *id != teo[0]).collect();
    if remaining.len() == 1 {
        return Err(DrawError::ProtectedPairImpossible);
    }
    let mut pairs = vec![(zayd[0], teo[0]), (teo[0], zayd[0])];
    if !remaining.is_empty() {
        pairs.extend(derangement(&remaining));
    }
    for (giver, receiver) in &pairs {
        sqlx::query("INSERT INTO assignments (giver_id, receiver_id) VALUES ($1, $2)")
            .bind(giver)
            .bind(receiver)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query(
        "INSERT INTO audit_log (actor_id, actor_name, action, details) VALUES ($1, $2, 'draw', $3)",
    )
    .bind(admin_id)
    .bind(admin_name)
    .bind(format!("Tirage effectué pour {} élèves", ids.len()))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(pairs.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn derangement_is_valid_for_many_sizes() {
        for n in [2usize, 3, 4, 10, 30, 100] {
            for _ in 0..200 {
                let ids: Vec<i64> = (1..=n as i64).map(|i| i * 7).collect();
                let pairs = derangement(&ids);
                assert_eq!(pairs.len(), n);
                assert!(pairs.iter().all(|(g, r)| g != r));
                let givers: HashSet<_> = pairs.iter().map(|p| p.0).collect();
                let receivers: HashSet<_> = pairs.iter().map(|p| p.1).collect();
                assert_eq!(givers, ids.iter().copied().collect());
                assert_eq!(receivers, ids.iter().copied().collect());
            }
        }
    }
}
