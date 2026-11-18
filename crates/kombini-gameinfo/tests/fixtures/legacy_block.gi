"GameInfo"
{
	game	"Deadlock"
	title	"Deadlock"

	FileSystem
	{	
		SteamAppId	1422450
		//
		// 3. If no "Mod" key, for the first "Game" search path, it adds a search path called "MOD".
		// 4. If no "Write" key, for the first "Game" search path, it adds a search path called "DEFAULT_WRITE_PATH".
		//
		SearchPaths
		{
			// These are optional language paths. They must be mounted first, which is why there are first in the list.
			Game_UILanguage		citadel_*LANGUAGE*

			// These are optional low-violence paths. They will only get mounted if you are in a low-violence mode.
			Game_LowViolence	citadel_lv

			// >>> Kombini Mod Manager (managed block) >>>
			Game citadel/addons
			// <<< Kombini Mod Manager <<<
			Game				citadel
			Game				core
		}

		"UserSettingsPathID"		"USRLOCAL"
		"LegacyUserSettingsPathID"	"MOD"
	}
}
