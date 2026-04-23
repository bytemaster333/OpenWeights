import { describe, expect, it } from "vitest"
import { ApiError } from "./api"

describe("ApiError", () => {
  it("captures status + code + optional requestId", () => {
    const err = new ApiError(403, "signed_url_expired", "req-abc")
    expect(err.status).toBe(403)
    expect(err.code).toBe("signed_url_expired")
    expect(err.requestId).toBe("req-abc")
    expect(err.message).toBe("403 signed_url_expired")
  })

  it("tolerates missing requestId", () => {
    const err = new ApiError(401, "session_expired")
    expect(err.requestId).toBeUndefined()
  })
})
