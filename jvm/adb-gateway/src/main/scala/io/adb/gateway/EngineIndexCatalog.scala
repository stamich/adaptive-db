package io.adb.gateway

import io.adb.catalog.{IndexCatalog, IndexInfo}
import io.adb.ffm.NativeDatabase

/** The engine's indexes (`adb_list_indexes_json`), read on every call: the listing is one cheap
  * native call, and reading it fresh means a planner never uses an index another client has
  * dropped meanwhile (an index dropped *during* a query makes the query fail, never return a
  * partial result).
  *
  * @param listing the engine's listing document
  */
final class EngineIndexCatalog(listing: () => String) extends IndexCatalog:
  /** Decodes the current listing. */
  def indexes: Vector[IndexInfo] = IndexCatalog.decode(listing())

/** Construction from a native database. */
object EngineIndexCatalog:
  /** A catalog reading `native`. */
  def apply(native: NativeDatabase): EngineIndexCatalog = new EngineIndexCatalog(() => native.listIndexesJson())
