# DOC-16 §4：错误码、消息和业务详情共用 UP/1 错误通道。
class CordisError(Exception):
    """DOC-16 §4：携带错误编号、消息与业务详情。"""

    def __init__(self, code, message, detail=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.detail = detail

    def wire(self):
        return {"code": self.code, "message": self.message, "detail": self.detail}
