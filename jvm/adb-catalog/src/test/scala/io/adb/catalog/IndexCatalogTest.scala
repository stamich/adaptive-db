package io.adb.catalog

import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The engine's index listing and the read-only index catalog. */
final class IndexCatalogTest:
  /** A listing as `adb_list_indexes_json` writes it. */
  private val listing =
    """{"indexes":[{"id":0,"name":"idx_balance","entity_id":3,"columns":[{"field_id":2,"direction":"asc"},""" +
      """{"field_id":4,"direction":"desc"}],"include":[3],"state":"active","created_at_ts":12,"usable_from_ts":12,"watermark_ts":40},""" +
      """{"id":2,"name":"idx_other","entity_id":5,"columns":[{"field_id":1}],"include":[],"state":"active",""" +
      """"created_at_ts":1,"usable_from_ts":30,"watermark_ts":40}]}"""

  /** Statuses decode with their columns, directions and snapshot range. */
  @Test def decodesTheEngineListing(): Unit =
    val indexes = IndexCatalog.decode(listing)
    assertEquals(2, indexes.size)
    val first = indexes.head
    assertEquals(IndexId(0), first.id)
    assertEquals(Vector(IndexKeyColumn(FieldId(2), false), IndexKeyColumn(FieldId(4), true)), first.columns)
    assertTrue(first.covers(FieldId(3)) && first.covers(FieldId(4)) && !first.covers(FieldId(1)))
    assertTrue(first.serves(12) && first.serves(40) && !first.serves(11) && !first.serves(41))
    assertFalse(indexes(1).serves(20), "vacuumed history is not served")

  /** Lookups by entity and by name (case-insensitive). */
  @Test def looksUpIndexes(): Unit =
    val catalog = IndexCatalog.of(IndexCatalog.decode(listing)*)
    assertEquals(Vector("idx_balance"), catalog.indexes(EntityId(3)).map(_.name))
    assertEquals(Some(IndexId(2)), catalog.index("IDX_OTHER").map(_.id))
    assertEquals(None, catalog.index("missing"))
    assertTrue(IndexCatalog.Empty.indexes.isEmpty)

  /** Malformed listings are rejected. */
  @Test def rejectsMalformedListings(): Unit =
    for bad <- Vector(
        "[]",
        """{"indexes":{}}""",
        listing.replace("\"direction\":\"desc\"", "\"direction\":\"up\""),
        listing.replace("\"usable_from_ts\":12", "\"usable_from_ts\":-1"),
        listing.replace("\"columns\":[{\"field_id\":1}]", "\"columns\":[]"),
        listing.replace("\"name\":\"idx_other\"", "\"name\":\"\"")
      )
    do assertThrows(classOf[IllegalArgumentException], () => { IndexCatalog.decode(bad); () }, bad)

  /** Truncated and byte-mutated listings fail with `IllegalArgumentException` only (never a
    * `ClassCastException`, `NoSuchElementException` or similar), or decode to well-formed
    * indexes.
    */
  @Test def mutatedListingsFailCleanly(): Unit =
    val random = new scala.util.Random(2300)
    val specials = "{}[]\":,-0123456789eE.\\ ad".toCharArray
    val mutations = (0 until listing.length).map(listing.substring(0, _)) ++ (0 until 5000).map { _ =>
      val chars = listing.toCharArray
      (0 to random.nextInt(3)).foreach(_ => chars(random.nextInt(chars.length)) = specials(random.nextInt(specials.length)))
      new String(chars)
    }
    mutations.foreach { text =>
      try IndexCatalog.decode(text).foreach(info => assertTrue(info.columns.nonEmpty && info.name.nonEmpty))
      catch case _: IllegalArgumentException => ()
    }
