package io.adb.advisor

/** Keeps the advice stable. A finding is shown only after it was made in `confirmAfter`
  * evaluations in a row, and is withdrawn only after it was missing in `withdrawAfter` in a row, so
  * a workload that wobbles around a threshold does not make the advice flicker.
  *
  * An advice that has been carried out (see the `applied` test of [[update]]) is withdrawn at once.
  * The object holds the only mutable state of the advisor's decisions; it is thread-safe.
  *
  * @param confirmAfter  consecutive evaluations that must find a candidate before it is shown (at least 1)
  * @param withdrawAfter consecutive evaluations that must miss a shown recommendation before it is withdrawn (at least 1)
  */
final class Hysteresis(confirmAfter: Int, withdrawAfter: Int):
  require(confirmAfter >= 1 && withdrawAfter >= 1, "confirmAfter and withdrawAfter must be at least 1")

  /** Progress of one finding. */
  private final class Track(var latest: Recommendation, var seen: Int, var missed: Int, var shown: Boolean)

  /** Findings by key. */
  private val tracks = scala.collection.mutable.LinkedHashMap.empty[String, Track]

  /** Feeds one evaluation's findings.
    *
    * @param found   the detectors' findings of this evaluation
    * @param applied tells whether a shown recommendation has been carried out since
    * @return the state changes this evaluation caused, in a stable order
    */
  def update(found: Vector[Recommendation], applied: Recommendation => Boolean): Vector[(AuditKind, Recommendation)] = synchronized {
    val changes = Vector.newBuilder[(AuditKind, Recommendation)]
    val now = found.map(r => r.key -> r).toMap

    // Findings made now: count them, refresh the shown text, confirm when long enough.
    found.foreach { r =>
      val track = tracks.getOrElseUpdate(r.key, new Track(r, 0, 0, false))
      track.latest = r
      track.seen += 1
      track.missed = 0
      if !track.shown && track.seen >= confirmAfter then
        track.shown = true
        changes += ((AuditKind.Confirmed, r))
    }

    // Findings not made now: a candidate that was not confirmed starts over; a shown one counts a miss.
    tracks.toVector.foreach { case (key, track) =>
      if !now.contains(key) then
        if !track.shown then
          tracks.remove(key)
        else
          track.missed += 1
          if track.missed >= withdrawAfter then
            tracks.remove(key)
            changes += ((AuditKind.Withdrawn, track.latest))
    }

    // Shown advice that has been carried out leaves at once, whatever the detectors say.
    tracks.toVector.foreach { case (key, track) =>
      if track.shown && applied(track.latest) then
        tracks.remove(key)
        changes += ((AuditKind.Applied, track.latest))
    }
    changes.result()
  }

  /** The recommendations shown now, in the order they were first found. */
  def shown: Vector[Recommendation] = synchronized(tracks.valuesIterator.filter(_.shown).map(_.latest).toVector)

  /** Forgets every finding. */
  def clear(): Unit = synchronized(tracks.clear())
