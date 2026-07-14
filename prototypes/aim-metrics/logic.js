'use strict';

// PROTOTYPE: pure metric comparison logic for issue 21. Delete or absorb after
// the definitions have been accepted. This module performs no I/O.

const NON_FIREARM_DAMAGE = new Set([
  '',
  'decoy',
  'flashbang',
  'hegrenade',
  'incgrenade',
  'inferno',
  'molotov',
  'smokegrenade',
]);

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

function round6(value) {
  return Number(value.toFixed(6));
}

function canonicalWeapon(value) {
  const weapon = String(value || '').replace(/^weapon_/, '');
  if (weapon === 'm4a1_silencer') return 'm4a1';
  if (weapon === 'usp_silencer') return 'hkp2000';
  return weapon;
}

function isEnemyFirearmHurt(event) {
  const attacker = event.attacker_steamid;
  const victim = event.user_steamid;
  const weapon = canonicalWeapon(event.weapon);
  return Boolean(attacker && victim && attacker !== victim && !NON_FIREARM_DAMAGE.has(weapon));
}

function percentile(sorted, fraction) {
  if (sorted.length === 0) return null;
  return sorted[Math.floor((sorted.length - 1) * fraction)];
}

function distribution(values) {
  if (values.length === 0) {
    return { count: 0, min: null, median: null, p90: null, p95: null, max: null, mean: null };
  }
  const sorted = [...values].sort((a, b) => a - b);
  const mean = sorted.reduce((sum, value) => sum + value, 0) / sorted.length;
  return {
    count: sorted.length,
    min: round6(sorted[0]),
    median: round6(percentile(sorted, 0.5)),
    p90: round6(percentile(sorted, 0.9)),
    p95: round6(percentile(sorted, 0.95)),
    max: round6(sorted[sorted.length - 1]),
    mean: round6(mean),
  };
}

function wrapDegrees(value) {
  return ((value + 180) % 360 + 360) % 360 - 180;
}

function roundAtTick(tick, sortedRoundEnds) {
  const index = sortedRoundEnds.findIndex((roundEnd) => roundEnd.tick >= tick);
  return index === -1 ? null : index + 1;
}

function buildPseudonyms(events, tickRows) {
  const ids = new Set();
  for (const event of events) {
    for (const key of ['attacker_steamid', 'assister_steamid', 'user_steamid']) {
      if (event[key]) ids.add(String(event[key]));
    }
  }
  for (const row of tickRows) {
    if (row.steamid) ids.add(String(row.steamid));
  }
  return new Map(
    [...ids]
      .sort()
      .map((steamid, index) => [steamid, `player-${String(index + 1).padStart(2, '0')}`]),
  );
}

function analyseAimMetrics(input) {
  const {
    source,
    playerHurts,
    weaponFires,
    fireBullets,
    playerBulletHits,
    roundEnds,
    tickRows,
  } = input;

  const sortedRoundEnds = [...roundEnds].sort((a, b) => a.tick - b.tick);
  const firearmHurts = playerHurts
    .map((event, rawHurtOrdinal) => ({ ...event, rawHurtOrdinal }))
    .filter(isEnemyFirearmHurt);
  const pseudonyms = buildPseudonyms([...playerHurts, ...weaponFires], tickRows);
  const player = (steamid) => pseudonyms.get(String(steamid)) || null;

  const tickState = new Map();
  for (const row of tickRows) {
    tickState.set(`${row.tick}|${row.steamid}`, row);
  }

  const crosshairRecords = [];
  const crosshairMissingState = [];
  firearmHurts.forEach((hurt) => {
    const attacker = tickState.get(`${hurt.tick}|${hurt.attacker_steamid}`);
    const victim = tickState.get(`${hurt.tick}|${hurt.user_steamid}`);
    const required = attacker && victim
      ? [attacker.X, attacker.Y, attacker.yaw, victim.X, victim.Y]
      : [];
    if (!attacker || !victim || !required.every(Number.isFinite)) {
      crosshairMissingState.push({
        hurt_ordinal: hurt.rawHurtOrdinal,
        tick: hurt.tick,
        attacker: player(hurt.attacker_steamid),
        victim: player(hurt.user_steamid),
      });
      return;
    }

    const targetBearing = Math.atan2(victim.Y - attacker.Y, victim.X - attacker.X) * 180 / Math.PI;
    const signedError = wrapDegrees(targetBearing - attacker.yaw);
    crosshairRecords.push({
      receipt_id: `hurt-${hurt.rawHurtOrdinal}`,
      round: roundAtTick(hurt.tick, sortedRoundEnds),
      tick: hurt.tick,
      attacker: player(hurt.attacker_steamid),
      victim: player(hurt.user_steamid),
      weapon: canonicalWeapon(hurt.weapon),
      hitgroup: hurt.hitgroup || null,
      attacker_xy: [round6(attacker.X), round6(attacker.Y)],
      victim_xy: [round6(victim.X), round6(victim.Y)],
      attacker_yaw_deg: round6(attacker.yaw),
      victim_pawn_bearing_deg: round6(targetBearing),
      signed_horizontal_error_deg: round6(signedError),
      absolute_horizontal_error_deg: round6(Math.abs(signedError)),
    });
  });

  const firearmFires = weaponFires
    .map((event, fireOrdinal) => ({
      ...event,
      fireOrdinal,
      canonical_weapon: canonicalWeapon(event.weapon),
    }))
    .filter((event) => event.user_steamid && !NON_FIREARM_DAMAGE.has(event.canonical_weapon));

  const fireByExactKey = new Map();
  for (const fire of firearmFires) {
    const key = `${fire.tick}|${fire.user_steamid}|${fire.canonical_weapon}`;
    if (!fireByExactKey.has(key)) fireByExactKey.set(key, []);
    fireByExactKey.get(key).push(fire);
  }

  const shotToHurtRecords = firearmHurts.map((hurt) => {
    const weapon = canonicalWeapon(hurt.weapon);
    const key = `${hurt.tick}|${hurt.attacker_steamid}|${weapon}`;
    const candidates = fireByExactKey.get(key) || [];
    return {
      receipt_id: `hurt-${hurt.rawHurtOrdinal}`,
      round: roundAtTick(hurt.tick, sortedRoundEnds),
      tick: hurt.tick,
      attacker: player(hurt.attacker_steamid),
      victim: player(hurt.user_steamid),
      weapon,
      exact_tick_fire_candidates: candidates.length,
      matched_fire_ordinal: candidates.length === 1 ? candidates[0].fireOrdinal : null,
      tick_offset: candidates.length === 1 ? 0 : null,
    };
  });

  const matchedShotRecords = shotToHurtRecords.filter((record) => record.matched_fire_ordinal !== null);
  const fireUse = new Map();
  for (const record of matchedShotRecords) {
    const ordinal = record.matched_fire_ordinal;
    fireUse.set(ordinal, (fireUse.get(ordinal) || 0) + 1);
  }
  const reusedFireCounts = [...fireUse.values()].filter((count) => count > 1);

  const crosshairErrors = crosshairRecords.map((record) => record.absolute_horizontal_error_deg);
  const exactCandidateCounts = shotToHurtRecords.map((record) => record.exact_tick_fire_candidates);

  return {
    prototype: {
      marker: 'THROWAWAY PROTOTYPE: issue 21',
      question: 'Which crosshair, time-to-damage, and spray definitions are honest with the pinned parser and bundled Demo?',
      privacy: 'Only the upstream bundled fixture is read; names and SteamIDs are replaced with within-Demo pseudonyms.',
    },
    source,
    decisions: {
      crosshair_placement: 'WITHHOLD',
      time_to_damage: 'WITHHOLD',
      spray_accuracy: 'WITHHOLD',
      diagnostic_only: [
        'Hurt-tick horizontal pawn-bearing error (successful firearm damage only)',
        'Exact-tick weapon_fire/player_hurt co-occurrence',
      ],
    },
    crosshair: {
      public_metric: null,
      truthful_label: 'Hurt-tick horizontal pawn-bearing error: diagnostic, hits only',
      definition: 'abs(wrapDegrees(bearing(attacker XY, victim pawn XY) - attacker yaw)) at each firearm player_hurt tick',
      why_withheld: [
        'The Demo has no externally validated map geometry, hitboxes, or line-of-sight result.',
        'The sample is conditioned on successful damage and cannot represent misses or pre-aim.',
        'Hurt-tick yaw is not an exposure-start or rendered-crosshair observation.',
        'Pawn-origin bearing is not hitbox or hit-location bearing.',
      ],
      error_bound: 'No finite accuracy bound is supported by the available evidence; report raw degrees only as a diagnostic.',
      empirical: {
        eligible_firearm_hurts: firearmHurts.length,
        measured_records: crosshairRecords.length,
        missing_tick_state: crosshairMissingState.length,
        absolute_horizontal_error_deg: distribution(crosshairErrors),
      },
      receipt_inputs: [
        'Demo SHA-256 and parser/protobuf identity',
        'hurt event ordinal, round, tick, weapon, hitgroup, attacker and victim pseudonyms',
        'attacker X/Y/yaw and victim pawn X/Y at the same Demo tick',
        'angle convention and formula version',
      ],
      records: crosshairRecords,
      excluded_missing_state: crosshairMissingState,
    },
    time_to_damage: {
      public_metric: null,
      truthful_label: 'Same-tick shot/hurt co-occurrence: diagnostic, not reaction time',
      definition: 'A firearm player_hurt is paired only when exactly one same-attacker, canonical-weapon weapon_fire exists at the identical Demo tick.',
      why_withheld: [
        'Exposure or first-visibility time is unavailable without geometry and line of sight.',
        'Subtick order is absent, so a zero-tick offset is not a zero-duration reaction.',
        'Projectile penetration can associate one fire event with multiple hurt events.',
      ],
      error_bound: 'For a zero-tick pair, within-tick order and duration are unknown; no millisecond conversion is published because the header exposes no validated tick interval.',
      empirical: {
        eligible_firearm_hurts: firearmHurts.length,
        exact_tick_unique_pairs: matchedShotRecords.length,
        unmatched_or_ambiguous: shotToHurtRecords.length - matchedShotRecords.length,
        ambiguous_candidate_records: exactCandidateCounts.filter((count) => count > 1).length,
        distinct_matched_fire_events: fireUse.size,
        reused_fire_events: reusedFireCounts.length,
        maximum_hurts_per_fire: fireUse.size === 0 ? 0 : Math.max(...fireUse.values()),
        tick_offset: distribution(matchedShotRecords.map((record) => record.tick_offset)),
      },
      receipt_inputs: [
        'Demo SHA-256 and parser/protobuf identity',
        'weapon_fire and player_hurt ordinals and raw ticks',
        'attacker, victim, canonical weapon alias, round, and pairing rule version',
        'all same-tick pairing candidates, including rejected ambiguity',
      ],
      records: shotToHurtRecords,
    },
    spray: {
      public_metric: null,
      truthful_label: null,
      candidate_definition: 'Pair fire_bullets shot seeds/recoil state to player_bullet_hit impacts, then compare every shot in a burst, including misses.',
      why_withheld: [
        'The representative bundled Demo exposes zero fire_bullets and zero player_bullet_hit records through the pinned parser.',
        'weapon_fire/player_hurt cannot count misses, reconstruct bullet paths, or distinguish every pellet and penetration.',
        'A damage-per-shot ratio would be an outcome statistic, not spray accuracy or recoil control.',
      ],
      error_bound: 'Unbounded because the required per-shot impact evidence is absent.',
      empirical: {
        weapon_fire_events: weaponFires.length,
        player_hurt_events: playerHurts.length,
        firearm_hurt_events: firearmHurts.length,
        fire_bullets_events: fireBullets.length,
        player_bullet_hit_events: playerBulletHits.length,
      },
      receipt_inputs_if_later_supported: [
        'fire_bullets tick, shooter, origin, angles, weapon, seed, inaccuracy, spread, recoil index, bullets remaining',
        'player_bullet_hit tick, attacker/victim slots, victim position, hitgroup, damage, penetration count, kill flag',
        'burst grouping rule, weapon metadata version, parser/protobuf identity, and unmatched-event ledger',
      ],
      records: [],
    },
    testable_invariants: [
      'No public metric is emitted when its required evidence is absent.',
      'Names and raw SteamIDs never appear in output.',
      'Every diagnostic record points to a deterministic event ordinal and tick.',
      'Crosshair diagnostics include successful enemy firearm damage only.',
      'Shot/hurt pairing requires one and only one exact-tick attacker/weapon candidate.',
      'Identical source bytes and parser identity produce identical canonical JSON and fingerprint.',
    ],
  };
}

module.exports = {
  analyseAimMetrics,
  canonicalWeapon,
  distribution,
  isEnemyFirearmHurt,
  wrapDegrees,
};
