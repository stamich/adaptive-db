#ifndef ADAPTIVE_DB_H
#define ADAPTIVE_DB_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Pointer/handle safety contract:
 * - every pointer range must reference readable/writable memory for the declared length;
 * - each owned handle returned by this API must be released exactly once;
 * - using or releasing a handle after release is undefined behavior;
 * - adb_last_error_ptr() is borrowed thread-local storage invalidated by the next error.
 */

typedef struct AdbDatabaseHandle AdbDatabaseHandle;
typedef struct AdbQueryHandle AdbQueryHandle;
typedef struct AdbBatchHandle AdbBatchHandle;

typedef enum AdbStatus {
    ADB_OK = 0,
    ADB_END_OF_STREAM = 1,
    ADB_INVALID_ARGUMENT = 2,
    ADB_CONFLICT = 3,
    ADB_IO_ERROR = 4,
    ADB_CORRUPTION = 5,
    ADB_CANCELLED = 6,
    ADB_INTERNAL = 255
} AdbStatus;

uint32_t adb_abi_version(void);
const uint8_t* adb_engine_version(size_t* out_len);

AdbStatus adb_open(
    const uint8_t* path_ptr,
    size_t path_len,
    AdbDatabaseHandle** out_db
);

AdbStatus adb_close(AdbDatabaseHandle* db);

AdbStatus adb_execute_plan_json(
    AdbDatabaseHandle* db,
    const uint8_t* plan_ptr,
    size_t plan_len,
    AdbQueryHandle** out_query
);

AdbStatus adb_query_next_batch(
    AdbQueryHandle* query,
    AdbBatchHandle** out_batch
);

AdbStatus adb_query_cancel(AdbQueryHandle* query);
AdbStatus adb_query_close(AdbQueryHandle* query);

const uint8_t* adb_batch_data(const AdbBatchHandle* batch);
size_t adb_batch_len(const AdbBatchHandle* batch);
AdbStatus adb_batch_release(AdbBatchHandle* batch);

const uint8_t* adb_last_error_ptr(void);
size_t adb_last_error_len(void);

#ifdef __cplusplus
}
#endif

#endif
