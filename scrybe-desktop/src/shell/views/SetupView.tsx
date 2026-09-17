import { SetupWizard } from "../../setup/SetupWizard";
import { useNavigate } from "../navigation";

/**
 * Guided setup, as a destination in the shell.
 *
 * A thin adapter: the wizard knows how to leave, the shell knows where
 * leaving goes, and neither knows the other's business.
 */
export function SetupView() {
  const navigate = useNavigate();
  return (
    <SetupWizard
      onExit={() => {
        navigate("sessions");
      }}
    />
  );
}
