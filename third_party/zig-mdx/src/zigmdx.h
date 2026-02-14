#ifndef ZIGMDX_H
#define ZIGMDX_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum zigmdx_status {
  ZIGMDX_STATUS_OK = 0,
  ZIGMDX_STATUS_PARSE_ERROR = 1,
  ZIGMDX_STATUS_INVALID_UTF8 = 2,
  ZIGMDX_STATUS_ALLOC_ERROR = 3,
  ZIGMDX_STATUS_INTERNAL_ERROR = 4,
};

int32_t zigmdx_parse_json(
    const uint8_t *input_ptr,
    size_t input_len,
    uint8_t **out_json_ptr,
    size_t *out_json_len);

void zigmdx_free_json(uint8_t *ptr, size_t len);

uint32_t zigmdx_abi_version(void);

#ifdef __cplusplus
}
#endif

#endif
