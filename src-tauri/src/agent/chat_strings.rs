//! App-authored chat messages use the effective UI locale, independently of the
//! natural language chosen by the model for its own responses.

pub(crate) struct ChatStrings {
    pub cancelled: &'static str,
    pub incomplete_reply: &'static str,
    pub interrupted: &'static str,
    pub completed: &'static str,
    pub awaiting_answer: &'static str,
    pub awaiting_approval: &'static str,
}

pub(crate) fn for_locale(locale: &str) -> &'static ChatStrings {
    match locale {
        "zh-CN" => &ChatStrings {
            cancelled: "已取消",
            incomplete_reply: "本轮回复未完成：已取消",
            interrupted: "应用已重启。请先检查文件事务恢复结果，再继续。",
            completed: "Agent 任务已完成。",
            awaiting_answer: "Agent 正在等待你的回答。",
            awaiting_approval: "Agent 正在等待你的批准。",
        },
        "ja" => &ChatStrings {
            cancelled: "キャンセルされました",
            incomplete_reply: "応答は完了していません：キャンセルされました",
            interrupted: "アプリが再起動しました。ファイル操作の復旧結果を確認してから続行してください。",
            completed: "エージェントのタスクが完了しました。",
            awaiting_answer: "エージェントが回答を待っています。",
            awaiting_approval: "エージェントが承認を待っています。",
        },
        "ko" => &ChatStrings {
            cancelled: "취소됨",
            incomplete_reply: "응답이 완료되지 않았습니다: 취소됨",
            interrupted: "앱이 다시 시작되었습니다. 계속하기 전에 파일 작업 복구 결과를 확인하세요.",
            completed: "에이전트 작업이 완료되었습니다.",
            awaiting_answer: "에이전트가 답변을 기다리고 있습니다.",
            awaiting_approval: "에이전트가 승인을 기다리고 있습니다.",
        },
        "de" => &ChatStrings {
            cancelled: "Abgebrochen",
            incomplete_reply: "Antwort unvollständig: abgebrochen",
            interrupted: "Die App wurde neu gestartet. Prüfe vor dem Fortfahren die Wiederherstellung der Dateioperationen.",
            completed: "Der Agent hat die Aufgabe abgeschlossen.",
            awaiting_answer: "Der Agent wartet auf deine Antwort.",
            awaiting_approval: "Der Agent wartet auf deine Freigabe.",
        },
        "fr" => &ChatStrings {
            cancelled: "Annulé",
            incomplete_reply: "Réponse incomplète : annulée",
            interrupted: "L’application a redémarré. Vérifiez le résultat de la récupération des opérations sur les fichiers avant de continuer.",
            completed: "L’agent a terminé la tâche.",
            awaiting_answer: "L’agent attend votre réponse.",
            awaiting_approval: "L’agent attend votre approbation.",
        },
        "es" => &ChatStrings {
            cancelled: "Cancelado",
            incomplete_reply: "Respuesta incompleta: cancelada",
            interrupted: "La aplicación se ha reiniciado. Comprueba el resultado de la recuperación de las operaciones con archivos antes de continuar.",
            completed: "El agente ha completado la tarea.",
            awaiting_answer: "El agente espera tu respuesta.",
            awaiting_approval: "El agente espera tu aprobación.",
        },
        "pt-BR" => &ChatStrings {
            cancelled: "Cancelado",
            incomplete_reply: "Resposta incompleta: cancelada",
            interrupted: "O aplicativo foi reiniciado. Confira o resultado da recuperação das operações com arquivos antes de continuar.",
            completed: "O agente concluiu a tarefa.",
            awaiting_answer: "O agente aguarda sua resposta.",
            awaiting_approval: "O agente aguarda sua aprovação.",
        },
        "ru" => &ChatStrings {
            cancelled: "Отменено",
            incomplete_reply: "Ответ не завершён: отменено",
            interrupted: "Приложение перезапущено. Прежде чем продолжить, проверьте результат восстановления файловых операций.",
            completed: "Агент завершил задачу.",
            awaiting_answer: "Агент ждёт вашего ответа.",
            awaiting_approval: "Агент ждёт вашего одобрения.",
        },
        "it" => &ChatStrings {
            cancelled: "Annullato",
            incomplete_reply: "Risposta incompleta: annullata",
            interrupted: "L’app è stata riavviata. Controlla il risultato del ripristino delle operazioni sui file prima di continuare.",
            completed: "L’agente ha completato l’attività.",
            awaiting_answer: "L’agente attende la tua risposta.",
            awaiting_approval: "L’agente attende la tua approvazione.",
        },
        _ => &ChatStrings {
            cancelled: "Cancelled",
            incomplete_reply: "Reply incomplete: cancelled",
            interrupted: "The app restarted. Check the file transaction recovery results before continuing.",
            completed: "The Agent completed the task.",
            awaiting_answer: "The Agent is waiting for your answer.",
            awaiting_approval: "The Agent is waiting for your approval.",
        },
    }
}
