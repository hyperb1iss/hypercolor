from enum import Enum


class ErrorCode(str, Enum):
    BAD_REQUEST = "bad_request"
    CONFLICT = "conflict"
    FORBIDDEN = "forbidden"
    INTERNAL_ERROR = "internal_error"
    NOT_FOUND = "not_found"
    PAYLOAD_TOO_LARGE = "payload_too_large"
    RATE_LIMITED = "rate_limited"
    UNAUTHORIZED = "unauthorized"
    UNSUPPORTED_MEDIA_TYPE = "unsupported_media_type"
    VALIDATION_ERROR = "validation_error"

    def __str__(self) -> str:
        return str(self.value)
